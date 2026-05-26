use itertools::Itertools;
use std::cmp::PartialEq;
use std::iter::Peekable;
use std::path::Path;

pub struct DiffSet<'a> {
    vec: Vec<Patch<'a>>,
}

pub struct PatchChange<'a> {
    search_line: Option<&'a str>,
    additions: Option<Vec<&'a str>>,
    removals: Option<Vec<&'a str>>,
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
        let (neg, pos, search) = lines
            .peeking_take_while(|line| !line.starts_with("***"))
            .try_fold(
                (Vec::new(), Vec::new(), None),
                |(mut neg, mut pos, search), line| match line.chars().next() {
                    Some(' ') => Ok((neg, pos, Some(line))),
                    Some('-') => {
                        neg.push(line);
                        Ok((neg, pos, search))
                    }
                    Some('+') => {
                        pos.push(line);
                        Ok((neg, pos, search))
                    }
                    _ => Err(anyhow::anyhow!("Invalid format for diff")),
                },
            )?;
        let neg = vec_to_option(neg);
        let pos = vec_to_option(pos);
        Ok(PatchChange {
            search_line: search,
            additions: pos,
            removals: neg,
        })
    }
}

fn vec_to_option(vec: Vec<&str>) -> Option<Vec<&str>> {
    if vec.is_empty() { None } else { Some(vec) }
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
                assert_eq!(changes.search_line, Some(" context"));
                assert_eq!(changes.removals, Some(vec!["-old"]));
                assert_eq!(changes.additions, Some(vec!["+new"]));
            }
            _ => panic!("expected update file patch"),
        }
        match &diff.vec[3] {
            Patch::MoveFile { from, to, changes } => {
                assert_eq!(*from, Path::new("old.txt"));
                assert_eq!(*to, Path::new("moved.txt"));
                assert_eq!(changes.search_line, None);
                assert_eq!(changes.removals, Some(vec!["-before"]));
                assert_eq!(changes.additions, Some(vec!["+after"]));
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
}
