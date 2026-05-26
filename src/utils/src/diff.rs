use itertools::Itertools;
use std::cmp::PartialEq;
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
            Some('-') => Ok(HunkLine::Context(line.strip_prefix('-').unwrap())),
            Some('+') => Ok(HunkLine::Context(line.strip_prefix('+').unwrap())),
            _ => Err(anyhow::anyhow!("Invalid syntax in hunk")),
        }
    }
}

pub struct PatchChange<'a> {
    hunks: Vec<HunkLine<'a>>,
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
        changes: PatchChange<'a>,
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

impl DiffSet<'_> {
    pub fn new<'a>(input: &'a str) -> anyhow::Result<DiffSet<'a>> {
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

    fn next_patch<'a>(
        lines: &mut Peekable<std::str::Lines<'a>>,
    ) -> anyhow::Result<Option<Patch<'a>>> {
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
                if hunk == "@@" {
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
                let hunk = lines
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("Invalid format for diff, no hunk"))?;
                if hunk == "@@" {
                    let changes = Self::get_patch_changes(lines)?;
                    Ok(Patch::MoveFile {
                        from: prefix.path,
                        changes,
                        to: prefix.a_path.unwrap(),
                    })
                } else {
                    Err(anyhow::anyhow!("Invalid format for diff, hunk different"))
                }
            }
        }?;

        Ok(Some(res))
    }

    fn get_patch_changes<'a>(
        lines: &mut Peekable<std::str::Lines<'a>>,
    ) -> anyhow::Result<PatchChange<'a>> {
        let res: Vec<HunkLine> = lines
            .peeking_take_while(|line| !line.starts_with("***"))
            .try_fold(Vec::new(), |mut acc, line: &str| {
                let hunk_line = HunkLine::new(line)?;
                acc.push(hunk_line);
                Ok::<Vec<HunkLine<'_>>, anyhow::Error>(acc)
            })?;

        if let Some(HunkLine::Add(add)) = res.first() {
            Err(anyhow::anyhow!("Cannot only have additions in patch"))
        } else {
            Ok(PatchChange { hunks: res })
        }
    }
}

fn vec_to_option(vec: Vec<&str>) -> Option<Vec<&str>> {
    if vec.is_empty() { None } else { Some(vec) }
}

fn apply_diff(base: &str, patch: Patch) -> anyhow::Result<String> {
    match patch {
        Patch::AddFile { diff, .. } => {
            let res = diff
                .into_iter()
                .fold(String::from(base), |b, x| b + "\n" + x);
            Ok(res)
        }
        Patch::UpdateFile { changes, .. } => {
            todo!()
        }
        Patch::MoveFile { from, to, changes } => todo!(),
        Patch::DeleteFile { .. } => Err(anyhow::anyhow!("No diff for delete lil bro")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_patch_type() {
        let input = "\
*** Begin Patch
*** Add File: new.txt
+one
+two
*** Delete File: gone.txt
*** Update File: changed.txt
@@
 context
-old
+new
*** Update File: old.txt
*** Move to: moved.txt
@@
-before
+after
*** End Patch";

        let diff = DiffSet::new(input).unwrap();

        assert_eq!(diff.vec.len(), 4);
        match &diff.vec[0] {
            Patch::AddFile { path, diff } => {
                assert_eq!(*path, Path::new("new.txt"));
                assert_eq!(*diff, vec!["one", "two"]);
            }
            _ => panic!("expected add file patch"),
        }
        match &diff.vec[1] {
            Patch::DeleteFile { path } => assert_eq!(*path, Path::new("gone.txt")),
            _ => panic!("expected delete file patch"),
        }
        match &diff.vec[2] {
            Patch::UpdateFile { path, changes } => {
                assert_eq!(*path, Path::new("changed.txt"));
                assert_eq!(changes.hunks.len(), 3);
                match &changes.hunks[..] {
                    [
                        HunkLine::Context("context"),
                        HunkLine::Context("old"),
                        HunkLine::Context("new"),
                    ] => {}
                    _ => panic!("expected parsed update hunk lines"),
                }
            }
            _ => panic!("expected update file patch"),
        }
        match &diff.vec[3] {
            Patch::MoveFile { from, to, changes } => {
                assert_eq!(*from, Path::new("old.txt"));
                assert_eq!(*to, Path::new("moved.txt"));
                assert_eq!(changes.hunks.len(), 2);
                match &changes.hunks[..] {
                    [HunkLine::Context("before"), HunkLine::Context("after")] => {}
                    _ => panic!("expected parsed move hunk lines"),
                }
            }
            _ => panic!("expected move file patch"),
        }
    }

    #[test]
    fn rejects_add_file_with_non_addition_line() {
        let input = "\
*** Begin Patch
*** Add File: new.txt
not an addition
*** End Patch";

        assert!(DiffSet::new(input).is_err());
    }

    #[test]
    fn rejects_patch_without_end_marker() {
        let input = "\
*** Begin Patch
*** Delete File: gone.txt";

        assert!(DiffSet::new(input).is_err());
    }

    #[test]
    fn parses_delete_file_as_last_patch() {
        let input = "\
*** Begin Patch
*** Delete File: gone.txt
*** End Patch";

        let diff = DiffSet::new(input).unwrap();

        assert_eq!(diff.vec.len(), 1);
        match &diff.vec[0] {
            Patch::DeleteFile { path } => assert_eq!(*path, Path::new("gone.txt")),
            _ => panic!("expected delete file patch"),
        }
    }

    #[test]
    fn rejects_content_after_end_marker() {
        let input = "\
*** Begin Patch
*** Delete File: gone.txt
*** End Patch
trailing content";

        assert!(DiffSet::new(input).is_err());
    }

    #[test]
    fn rejects_blank_line_in_update_hunk() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@

*** End Patch";

        assert!(DiffSet::new(input).is_err());
    }

    #[test]
    fn parses_update_hunk_starting_with_addition_as_context() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@
+new
*** End Patch";

        let diff = DiffSet::new(input).unwrap();

        match &diff.vec[0] {
            Patch::UpdateFile { changes, .. } => match &changes.hunks[..] {
                [HunkLine::Context("new")] => {}
                _ => panic!("expected parsed addition hunk line"),
            },
            _ => panic!("expected update file patch"),
        }
    }
}
