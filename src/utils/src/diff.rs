use itertools::Itertools;
use std::iter::Peekable;
use std::path::Path;

pub struct DiffSet<'a> {
    vec: Vec<Patch<'a>>,
}

pub enum HunkLine<'a> {
    Context(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

impl<'a> HunkLine<'a> {
    fn new(line: &'a str) -> anyhow::Result<HunkLine<'a>> {
        match line.chars().next() {
            Some(' ') => Ok(HunkLine::Context(line.strip_prefix(' ').unwrap())),
            Some('-') => Ok(HunkLine::Remove(line.strip_prefix('-').unwrap())),
            Some('+') => Ok(HunkLine::Add(line.strip_prefix('+').unwrap())),
            None => Ok(HunkLine::Context("")),
            _ => Err(anyhow::anyhow!("Invalid syntax in hunk")),
        }
    }
}

pub struct PatchChange<'a> {
    hunks: Vec<HunkBatch<'a>>,
}

struct HunkBatch<'a> {
    lines: Vec<HunkLine<'a>>,
}

impl<'a> HunkBatch<'a> {
    fn new(lines: Vec<HunkLine<'a>>) -> anyhow::Result<HunkBatch<'a>> {
        if lines.is_empty() {
            Err(anyhow::anyhow!("Update hunks cannot be empty"))
        } else if lines.iter().all(|line| matches!(line, HunkLine::Add(_))) {
            Err(anyhow::anyhow!(
                "Update hunks must include a context or removal line"
            ))
        } else {
            Ok(HunkBatch { lines })
        }
    }
}

pub enum Patch<'a> {
    AddFile {
        diff: Vec<&'a str>,
        path: &'a Path,
    },
    DeleteFile {
        path: &'a Path,
    },
    UpdateFile {
        path: &'a Path,
        changes: PatchChange<'a>,
    },
    MoveFile {
        from: &'a Path,
        to: &'a Path,
        changes: Option<PatchChange<'a>>,
    },
}

#[derive(Eq, PartialEq)]
enum PatchType {
    AddFile,
    DeleteFile,
    UpdateFile,
    MoveFile,
}
struct PatchPrefix<'a> {
    p_type: PatchType,
    path: &'a Path,
    a_path: Option<&'a Path>,
}

impl<'a> PatchPrefix<'a> {
    fn new(
        op: &str,
        params: &'a str,
        op_param: Option<(&str, &'a str)>,
    ) -> anyhow::Result<PatchPrefix<'a>> {
        match op {
            "Add File" => Ok(PatchPrefix {
                p_type: PatchType::AddFile,
                path: Path::new(params),
                a_path: None,
            }),
            "Delete File" => Ok(PatchPrefix {
                p_type: PatchType::DeleteFile,
                path: Path::new(params),
                a_path: None,
            }),
            "Update File" => match op_param {
                Some((a_op, a_param)) => {
                    if a_op == "Move to" {
                        Ok(PatchPrefix {
                            p_type: PatchType::MoveFile,
                            path: Path::new(params),
                            a_path: Some(Path::new(a_param)),
                        })
                    } else {
                        Err(anyhow::anyhow!(
                            "Invalid format for diff, Move to is the only additional update"
                        ))
                    }
                }
                None => Ok(PatchPrefix {
                    p_type: PatchType::UpdateFile,
                    path: Path::new(params),
                    a_path: None,
                }),
            },
            _ => Err(anyhow::anyhow!(
                "Invalid format for diff, addition param is only for updates"
            )),
        }
    }
}

impl<'a> DiffSet<'a> {
    pub fn new(input: &'a str) -> anyhow::Result<DiffSet<'a>> {
        let mut lines = input.lines().peekable();
        let header = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invalid format for diff"))?;

        if header == "*** Begin Patch" {
            let mut vec = Vec::new();
            while let Some(patch) = Self::next_patch(&mut lines)? {
                vec.push(patch);
            }
            if let Some(footer) = lines.next() {
                if footer == "*** End Patch" && lines.next().is_none() {
                    Ok(DiffSet { vec })
                } else {
                    Err(anyhow::anyhow!("Invalid format for diff"))
                }
            } else {
                Err(anyhow::anyhow!("Invalid format for diff"))
            }
        } else {
            Err(anyhow::anyhow!("Invalid format for diff"))
        }
    }

    pub fn patches(&self) -> &[Patch<'a>] {
        &self.vec
    }

    pub fn into_patches(self) -> Vec<Patch<'a>> {
        self.vec
    }

    fn next_patch(lines: &mut Peekable<std::str::Lines<'a>>) -> anyhow::Result<Option<Patch<'a>>> {
        let (op, param) = match lines.peek().and_then(|t| match t {
            &"*** End Patch" => None,
            _ => {
                let res = t
                    .strip_prefix("*** ")
                    .ok_or_else(|| anyhow::anyhow!("Invalid format for diff"))
                    .and_then(|rest| {
                        rest.split_once(":")
                            .map(|(a, b)| (a, b.trim()))
                            .ok_or_else(|| anyhow::anyhow!("Invalid format for diff"))
                    });
                Some(res)
            }
        }) {
            Some(res) => {
                // advance if it's not end patch
                lines.next();
                res
            }
            None => return Ok(None),
        }?;

        let a_pair = if op == "Update File" {
            lines
                .peek()
                .and_then(|line| match line.strip_prefix("*** ") {
                    // Move to op
                    Some(rest) => Some(
                        rest.split_once(":")
                            .map(|(a, b)| (a, b.trim()))
                            .ok_or_else(|| anyhow::anyhow!("Invalid format for diff")),
                    ),
                    None => None,
                })
                .transpose()?
        } else {
            None
        };

        let prefix = PatchPrefix::new(op, param, a_pair)?;

        let res = match prefix.p_type {
            PatchType::AddFile => {
                let diff: anyhow::Result<Vec<_>> =
                    lines
                        .peeking_take_while(|line| !line.starts_with("***"))
                        .map(|line| {
                            line.strip_prefix("+").ok_or_else(|| anyhow::anyhow!(
                                "Invalid format for diff, For add file only additions are supported"
                            ))
                        })
                        .collect();
                let diff = diff?;

                Ok(Patch::AddFile {
                    diff,
                    path: prefix.path,
                })
            }
            PatchType::DeleteFile => Ok(Patch::DeleteFile { path: prefix.path }),
            PatchType::UpdateFile => {
                let hunk = lines
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Invalid format for diff, no hunk"))?;
                if Self::is_hunk_header(hunk) {
                    let changes = Self::get_patch_changes(lines)?;
                    Ok(Patch::UpdateFile {
                        path: prefix.path,
                        changes,
                    })
                } else {
                    Err(anyhow::anyhow!("Invalid format for diff, hunk different"))
                }
            }
            PatchType::MoveFile => {
                match a_pair {
                    Some(_) => {
                        lines.next();
                    }
                    None => {}
                }
                let changes = match lines.peek().copied() {
                    Some(hunk) if Self::is_hunk_header(hunk) => {
                        lines.next();
                        Some(Self::get_patch_changes(lines)?)
                    }
                    _ => None,
                };
                Ok(Patch::MoveFile {
                    from: prefix.path,
                    changes,
                    to: prefix.a_path.unwrap(),
                })
            }
        }?;

        Ok(Some(res))
    }

    fn is_hunk_header(line: &str) -> bool {
        line == "@@" || line.starts_with("@@ ")
    }

    fn get_patch_changes(
        lines: &mut Peekable<std::str::Lines<'a>>,
    ) -> anyhow::Result<PatchChange<'a>> {
        let hunk_lines: Vec<&str> = lines
            .peeking_take_while(|line| !line.starts_with("***"))
            .collect();
        let hunks: Vec<HunkBatch<'a>> = hunk_lines
            .split(|line| Self::is_hunk_header(line))
            .map(|lines| {
                lines
                    .iter()
                    .copied()
                    .map(HunkLine::new)
                    .collect::<anyhow::Result<Vec<_>>>()
                    .and_then(HunkBatch::new)
            })
            .collect::<anyhow::Result<_>>()?;

        Ok(PatchChange { hunks })
    }
}

pub fn apply_diff(base: &str, patch: Patch<'_>) -> anyhow::Result<String> {
    match patch {
        Patch::AddFile { diff, .. } => {
            if base.is_empty() {
                Ok(diff.join("\n"))
            } else {
                Err(anyhow::anyhow!(
                    "Cannot apply an add-file patch to existing content"
                ))
            }
        }
        Patch::UpdateFile { changes, .. }
        | Patch::MoveFile {
            changes: Some(changes),
            ..
        } => apply_changes(base, changes),
        Patch::MoveFile { changes: None, .. } => Ok(base.to_owned()),
        Patch::DeleteFile { .. } => Err(anyhow::anyhow!(
            "Delete-file patches must be applied as a filesystem operation"
        )),
    }
}

fn apply_changes(base: &str, changes: PatchChange<'_>) -> anyhow::Result<String> {
    let lines: Vec<String> = base.trim().lines().map(str::to_owned).collect();
    let (lines, _) =
        changes
            .hunks
            .into_iter()
            .try_fold((lines, 0), |(mut lines, cursor), batch| {
                let source: Vec<&str> = batch
                    .lines
                    .iter()
                    .filter_map(|line| match line {
                        HunkLine::Context(text) | HunkLine::Remove(text) => Some(*text),
                        HunkLine::Add(_) => None,
                    })
                    .collect();
                let window_start = lines[cursor..]
                    .windows(source.len())
                    .position(|window| window.iter().map(String::as_str).eq(source.iter().copied()))
                    .map(|position| position + cursor)
                    .ok_or_else(|| anyhow::anyhow!("Patch hunk does not match the base content"))?;
                let replacement: Vec<String> = batch
                    .lines
                    .iter()
                    .filter_map(|line| match line {
                        HunkLine::Context(text) | HunkLine::Add(text) => Some((*text).to_owned()),
                        HunkLine::Remove(_) => None,
                    })
                    .collect();
                let replacement_len = replacement.len();

                lines.splice(window_start..window_start + source.len(), replacement);
                Ok::<(Vec<String>, usize), anyhow::Error>((lines, window_start + replacement_len))
            })?;
    let mut res = lines.join("\n");
    res.push('\n');
    Ok(res)
}

#[cfg(test)]
#[path = "../tests/unit/diff/tests.rs"]
mod tests;
