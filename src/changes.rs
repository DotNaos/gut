use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub files: Vec<String>,
    pub hunks: Vec<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.hunks.is_empty()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesResult {
    pub head: String,
    pub files: Vec<ChangeFile>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeFile {
    pub id: String,
    pub path: String,
    pub staged: Option<ChangeLayer>,
    pub unstaged: Option<ChangeLayer>,
    pub untracked: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeLayer {
    pub status: String,
    pub binary: bool,
    pub hunks: Vec<ChangeHunk>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeHunk {
    pub id: String,
    pub selectable: bool,
    pub header: String,
    pub old_start: u64,
    pub old_lines: u64,
    pub new_start: u64,
    pub new_lines: u64,
    pub lines: Vec<String>,
}

struct InternalFile {
    public: ChangeFile,
    staged: Option<LayerPatch>,
    unstaged: Option<LayerPatch>,
}

struct LayerPatch {
    full: String,
    header: String,
    status: String,
    binary: bool,
    hunks: Vec<InternalHunk>,
}

struct InternalHunk {
    public: ChangeHunk,
    patch: String,
}

struct UntrackedSnapshot {
    path: String,
    contents: Vec<u8>,
    #[cfg(not(unix))]
    readonly: bool,
    #[cfg(unix)]
    mode: u32,
}

pub struct SelectiveState {
    original_head: String,
    original_staged: String,
    original_unstaged: String,
    selected_staged: String,
    selected_unstaged: String,
    residual_staged: String,
    residual_unstaged: String,
    untracked: Vec<UntrackedSnapshot>,
    selected_untracked: HashSet<String>,
}

pub fn inspect() -> Result<ChangesResult, String> {
    let (head, files) = inspect_internal()?;
    Ok(ChangesResult {
        head,
        files: files.into_values().map(|file| file.public).collect(),
    })
}

pub fn prepare_selection(selection: &Selection) -> Result<SelectiveState, String> {
    if selection.is_empty() {
        return Err("selective change preparation requires --file or --hunk".to_owned());
    }

    let (head, files) = inspect_internal()?;
    let selected_files: HashSet<_> = selection.files.iter().cloned().collect();
    let selected_hunks: HashSet<_> = selection.hunks.iter().cloned().collect();

    let known_paths: HashSet<_> = files.keys().cloned().collect();
    for path in &selected_files {
        if !known_paths.contains(path) {
            return Err(format!("unknown changed file selector: {path}"));
        }
    }

    let mut known_hunks = HashSet::new();
    for file in files.values() {
        for layer in [&file.staged, &file.unstaged].into_iter().flatten() {
            for hunk in &layer.hunks {
                if hunk.public.selectable {
                    known_hunks.insert(hunk.public.id.clone());
                }
            }
        }
    }
    for hunk in &selected_hunks {
        if !known_hunks.contains(hunk) {
            return Err(format!("unknown hunk selector: {hunk}"));
        }
    }

    let mut original_staged = String::new();
    let mut original_unstaged = String::new();
    let mut selected_staged = String::new();
    let mut selected_unstaged = String::new();
    let mut residual_staged = String::new();
    let mut residual_unstaged = String::new();
    let mut selected_untracked = HashSet::new();

    for (path, file) in &files {
        let whole_file = selected_files.contains(path);
        if file.public.untracked && whole_file {
            selected_untracked.insert(path.clone());
        }

        if let Some(layer) = &file.staged {
            original_staged.push_str(&layer.full);
            split_layer(
                layer,
                whole_file,
                &selected_hunks,
                &mut selected_staged,
                &mut residual_staged,
            )?;
        }
        if let Some(layer) = &file.unstaged {
            original_unstaged.push_str(&layer.full);
            split_layer(
                layer,
                whole_file,
                &selected_hunks,
                &mut selected_unstaged,
                &mut residual_unstaged,
            )?;
        }
    }

    if selected_staged.is_empty() && selected_unstaged.is_empty() && selected_untracked.is_empty() {
        return Err("the requested selectors do not select any changes".to_owned());
    }

    let root = repository_root()?;
    let mut untracked = Vec::new();
    for file in files.values().filter(|file| file.public.untracked) {
        untracked.push(snapshot_untracked(&root, &file.public.path)?);
    }

    Ok(SelectiveState {
        original_head: head,
        original_staged,
        original_unstaged,
        selected_staged,
        selected_unstaged,
        residual_staged,
        residual_unstaged,
        untracked,
        selected_untracked,
    })
}

impl SelectiveState {
    pub fn original_head(&self) -> &str {
        &self.original_head
    }

    pub fn isolate_selected(&self) -> Result<(), String> {
        self.reset_to_original_head()?;
        self.clear_untracked()?;
        apply_patch(&self.selected_staged, true)?;
        apply_patch(&self.selected_unstaged, true)?;
        for snapshot in &self.untracked {
            if self.selected_untracked.contains(&snapshot.path) {
                snapshot.restore()?;
                git_ok(&["add", "--", &snapshot.path])?;
            }
        }
        Ok(())
    }

    pub fn preflight_residual(&self) -> Result<(), String> {
        if let Err(error) = self.restore_residual() {
            let restore = self.restore_original();
            return match restore {
                Ok(()) => Err(format!(
                    "selected changes cannot be isolated while preserving the remaining staged/unstaged state: {error}"
                )),
                Err(restore_error) => Err(format!(
                    "selected changes cannot be isolated: {error}; failed to restore original state: {restore_error}"
                )),
            };
        }
        self.isolate_selected()
    }

    pub fn restore_residual(&self) -> Result<(), String> {
        apply_patch(&self.residual_staged, true)?;
        apply_patch(&self.residual_unstaged, false)?;
        for snapshot in &self.untracked {
            if !self.selected_untracked.contains(&snapshot.path) {
                snapshot.restore()?;
            }
        }
        Ok(())
    }

    pub fn restore_original(&self) -> Result<(), String> {
        self.reset_to_original_head()?;
        self.clear_untracked()?;
        apply_patch(&self.original_staged, true)?;
        apply_patch(&self.original_unstaged, false)?;
        for snapshot in &self.untracked {
            snapshot.restore()?;
        }
        Ok(())
    }

    fn reset_to_original_head(&self) -> Result<(), String> {
        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        git_ok(&["reset", "--hard", &self.original_head])
    }

    fn clear_untracked(&self) -> Result<(), String> {
        let root = repository_root()?;
        for snapshot in &self.untracked {
            let path = root.join(&snapshot.path);
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("failed to remove {}: {error}", path.display()));
                }
            }
        }
        Ok(())
    }
}

fn inspect_internal() -> Result<(String, BTreeMap<String, InternalFile>), String> {
    let head = git_output(&["rev-parse", "HEAD"])?;
    let head = head.trim().to_owned();
    inspect_snapshot_patches(head)
}

fn inspect_snapshot_patches(
    head: String,
) -> Result<(String, BTreeMap<String, InternalFile>), String> {
    let mut paths = BTreeSet::new();
    paths.extend(diff_names(&["diff", "--cached", "--name-only", "-z"])?);
    paths.extend(diff_names(&["diff", "--name-only", "-z"])?);
    let untracked = diff_names(&["ls-files", "--others", "--exclude-standard", "-z"])?;
    paths.extend(untracked.iter().cloned());
    let untracked: HashSet<_> = untracked.into_iter().collect();

    let root = repository_root()?;
    let mut files = BTreeMap::new();
    for path in paths {
        let staged = layer_patch("staged", &path, true)?;
        let unstaged = layer_patch("unstaged", &path, false)?;
        let is_untracked = untracked.contains(&path);
        let untracked_hash = if is_untracked {
            let contents = fs::read(root.join(&path))
                .map_err(|error| format!("failed to read untracked {path}: {error}"))?;
            stable_bytes_hash(&contents)
        } else {
            String::new()
        };
        let file_id = stable_id(
            "f",
            &format!(
                "{path}\0{}\0{}\0{is_untracked}\0{untracked_hash}",
                staged
                    .as_ref()
                    .map(|layer| layer.full.as_str())
                    .unwrap_or(""),
                unstaged
                    .as_ref()
                    .map(|layer| layer.full.as_str())
                    .unwrap_or("")
            ),
        );
        let public = ChangeFile {
            id: file_id,
            path: path.clone(),
            staged: staged.as_ref().map(public_layer),
            unstaged: unstaged.as_ref().map(public_layer),
            untracked: is_untracked,
        };
        files.insert(
            path,
            InternalFile {
                public,
                staged,
                unstaged,
            },
        );
    }
    Ok((head, files))
}

fn public_layer(layer: &LayerPatch) -> ChangeLayer {
    ChangeLayer {
        status: layer.status.clone(),
        binary: layer.binary,
        hunks: layer.hunks.iter().map(|hunk| hunk.public.clone()).collect(),
    }
}

fn layer_patch(layer_name: &str, path: &str, staged: bool) -> Result<Option<LayerPatch>, String> {
    let mut status_args = vec!["diff"];
    if staged {
        status_args.push("--cached");
    }
    status_args.extend(["--name-status", "--no-renames", "--", path]);
    let status_output = git_output(&status_args)?;
    let Some(status) = status_output
        .lines()
        .find(|line| !line.is_empty())
        .and_then(|line| line.split_whitespace().next())
    else {
        return Ok(None);
    };

    let mut patch_args = vec!["diff"];
    if staged {
        patch_args.push("--cached");
    }
    patch_args.extend([
        "--binary",
        "--no-renames",
        "--no-ext-diff",
        "--no-color",
        "--unified=3",
        "--",
        path,
    ]);
    let full = git_output(&patch_args)?;
    let binary = full.contains("GIT binary patch") || full.contains("Binary files ");
    let (header, raw_hunks) = parse_patch(&full);
    let hunk_selectable = status == "M" && !binary;
    let hunks = raw_hunks
        .into_iter()
        .filter_map(|patch| {
            let header_line = patch.lines().next()?.to_owned();
            let (old_start, old_lines, new_start, new_lines) = parse_hunk_header(&header_line)?;
            let id = stable_id("h", &format!("{layer_name}\0{path}\0{patch}"));
            let lines = patch.lines().skip(1).map(str::to_owned).collect();
            Some(InternalHunk {
                public: ChangeHunk {
                    id,
                    selectable: hunk_selectable,
                    header: header_line,
                    old_start,
                    old_lines,
                    new_start,
                    new_lines,
                    lines,
                },
                patch,
            })
        })
        .collect();

    Ok(Some(LayerPatch {
        full,
        header,
        status: status.to_owned(),
        binary,
        hunks,
    }))
}

fn parse_patch(patch: &str) -> (String, Vec<String>) {
    let mut header = Vec::new();
    let mut hunks = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            if let Some(previous) = current.take() {
                hunks.push(previous.concat());
            }
            current = Some(vec![line.to_owned()]);
        } else if let Some(lines) = current.as_mut() {
            lines.push(line.to_owned());
        } else {
            header.push(line.to_owned());
        }
    }
    if let Some(last) = current {
        hunks.push(last.concat());
    }
    (header.concat(), hunks)
}

fn split_layer(
    layer: &LayerPatch,
    whole_file: bool,
    selected_hunks: &HashSet<String>,
    selected: &mut String,
    residual: &mut String,
) -> Result<(), String> {
    if whole_file {
        selected.push_str(&layer.full);
        return Ok(());
    }

    let selected_in_layer = layer
        .hunks
        .iter()
        .filter(|hunk| hunk.public.selectable && selected_hunks.contains(&hunk.public.id))
        .collect::<Vec<_>>();
    if selected_in_layer.is_empty() {
        residual.push_str(&layer.full);
        return Ok(());
    }
    if layer.binary || layer.status != "M" {
        return Err("hunk selection is only supported for modified text files; use --file for additions, deletions, type changes, or binary files".to_owned());
    }

    let filtered_header = filtered_partial_header(&layer.header);
    selected.push_str(&filtered_header);
    for hunk in &selected_in_layer {
        selected.push_str(&hunk.patch);
    }

    let remaining = layer
        .hunks
        .iter()
        .filter(|hunk| !hunk.public.selectable || !selected_hunks.contains(&hunk.public.id))
        .collect::<Vec<_>>();
    if !remaining.is_empty() {
        residual.push_str(&filtered_header);
        for hunk in remaining {
            residual.push_str(&hunk.patch);
        }
    }
    Ok(())
}

fn filtered_partial_header(header: &str) -> String {
    header
        .lines()
        .filter(|line| !line.starts_with("index "))
        .map(|line| format!("{line}\n"))
        .collect()
}

fn parse_hunk_header(line: &str) -> Option<(u64, u64, u64, u64)> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let mut ranges = rest[..end].split_whitespace();
    let old = ranges.next()?.strip_prefix('-')?;
    let new = ranges.next()?.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;
    Some((old_start, old_lines, new_start, new_lines))
}

fn parse_range(value: &str) -> Option<(u64, u64)> {
    match value.split_once(',') {
        Some((start, lines)) => Some((start.parse().ok()?, lines.parse().ok()?)),
        None => Some((value.parse().ok()?, 1)),
    }
}

fn stable_id(prefix: &str, value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{prefix}-{hash:016x}")
}

fn stable_bytes_hash(value: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn diff_names(args: &[&str]) -> Result<Vec<String>, String> {
    let output = git_bytes(args)?;
    output
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| {
            String::from_utf8(value.to_vec())
                .map_err(|_| "gut currently requires UTF-8 repository paths".to_owned())
        })
        .collect()
}

fn snapshot_untracked(root: &Path, path: &str) -> Result<UntrackedSnapshot, String> {
    let full = root.join(path);
    let metadata = fs::symlink_metadata(&full)
        .map_err(|error| format!("failed to inspect untracked {path}: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "selective placement currently supports regular untracked files only: {path}"
        ));
    }
    let contents =
        fs::read(&full).map_err(|error| format!("failed to snapshot untracked {path}: {error}"))?;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    Ok(UntrackedSnapshot {
        path: path.to_owned(),
        contents,
        #[cfg(not(unix))]
        readonly: metadata.permissions().readonly(),
        #[cfg(unix)]
        mode: metadata.permissions().mode(),
    })
}

impl UntrackedSnapshot {
    fn restore(&self) -> Result<(), String> {
        let root = repository_root()?;
        let path = root.join(&self.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(&path, &self.contents)
            .map_err(|error| format!("failed to restore {}: {error}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(self.mode)).map_err(|error| {
                format!(
                    "failed to restore permissions for {}: {error}",
                    path.display()
                )
            })?;
        }
        #[cfg(not(unix))]
        {
            let mut permissions = fs::metadata(&path)
                .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?
                .permissions();
            permissions.set_readonly(self.readonly);
            fs::set_permissions(&path, permissions).map_err(|error| {
                format!(
                    "failed to restore permissions for {}: {error}",
                    path.display()
                )
            })?;
        }
        Ok(())
    }
}

fn apply_patch(patch: &str, index: bool) -> Result<(), String> {
    if patch.is_empty() {
        return Ok(());
    }
    let mut command = Command::new("git");
    command.args(["apply", "--binary", "--whitespace=nowarn"]);
    if index {
        command.arg("--index");
    }
    command.stdin(Stdio::piped()).stdout(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run git apply: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "failed to open git apply stdin".to_owned())?
        .write_all(patch.as_bytes())
        .map_err(|error| format!("failed to write patch to git apply: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for git apply: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn repository_root() -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        git_output(&["rev-parse", "--show-toplevel"])?.trim(),
    ))
}

fn git_ok(args: &[&str]) -> Result<(), String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn git_output(args: &[&str]) -> Result<String, String> {
    String::from_utf8(git_bytes(args)?).map_err(|_| "git returned non-UTF-8 output".to_owned())
}

fn git_bytes(args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
}
