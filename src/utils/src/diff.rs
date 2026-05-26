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
        let res: Vec<HunkLine> = lines
            .peeking_take_while(|line| !line.starts_with("***"))
            .try_fold(Vec::new(), |mut acc, line: &str| {
                let hunk_line = HunkLine::new(line)?;
                acc.push(hunk_line);
                Ok::<Vec<HunkLine<'_>>, anyhow::Error>(acc)
            })?;

        if res.is_empty() {
            Err(anyhow::anyhow!("Update hunks cannot be empty"))
        } else if res.iter().all(|line| matches!(line, HunkLine::Add(_))) {
            Err(anyhow::anyhow!(
                "Update hunks must include a context or removal line"
            ))
        } else {
            Ok(PatchChange { hunks: res })
        }
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
    let source: Vec<&str> = changes
        .hunks
        .iter()
        .filter_map(|line| match line {
            HunkLine::Context(text) | HunkLine::Remove(text) => Some(*text),
            HunkLine::Add(_) => None,
        })
        .collect();

    let lines: Vec<&str> = base.trim().lines().collect();

    let window_start = lines
        .windows(source.len())
        .position(|window| window == source.as_slice())
        .ok_or_else(|| anyhow::anyhow!("Patch hunk does not match the base content"))?;

    let replacement = changes.hunks.iter().filter_map(|line| match line {
        HunkLine::Context(text) | HunkLine::Add(text) => Some(*text),
        HunkLine::Remove(_) => None,
    });

    let mut res = Vec::with_capacity(lines.len() + changes.hunks.len());
    res.extend_from_slice(&lines[..window_start]);
    res.extend(replacement);
    res.extend_from_slice(&lines[window_start + source.len()..]);

    let mut res = res.join("\n");
    res.push('\n');
    Ok(res)
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
                        HunkLine::Remove("old"),
                        HunkLine::Add("new"),
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
                let changes = changes.as_ref().unwrap();
                assert_eq!(changes.hunks.len(), 2);
                match &changes.hunks[..] {
                    [HunkLine::Remove("before"), HunkLine::Add("after")] => {}
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
    fn applies_update_with_blank_context_lines() {
        let input = "\
*** Begin Patch
*** Update File: src/main.rs
@@
 fn main() {
-    let x = 1;
+    let x = 10;

     println!(\"{x}\");

-    let y = 2;
+    let y = 20;
 }
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(
            apply_diff(
                "fn main() {\n    let x = 1;\n\n    println!(\"{x}\");\n\n    let y = 2;\n}",
                patch
            )
            .unwrap(),
            "fn main() {\n    let x = 10;\n\n    println!(\"{x}\");\n\n    let y = 20;\n}\n"
        );
    }

    #[test]
    fn applies_multiple_changes_in_single_hunk() {
        let input = "\
*** Begin Patch
*** Update File: src/main.rs
@@
 fn main() {
-    let x = 1;
+    let x = 10;
     println!(\"{x}\");
-    let y = 2;
+    let y = 20;
 }
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(
            apply_diff(
                "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n    let y = 2;\n}",
                patch
            )
            .unwrap(),
            "fn main() {\n    let x = 10;\n    println!(\"{x}\");\n    let y = 20;\n}\n"
        );
    }

    #[test]
    fn rejects_update_hunk_without_anchor() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@
+new
*** End Patch";

        assert!(DiffSet::new(input).is_err());
    }

    #[test]
    fn applies_add_file_without_a_leading_newline() {
        let input = "\
*** Begin Patch
*** Add File: new.txt
+one
+two
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(apply_diff("", patch).unwrap(), "one\ntwo");
    }

    #[test]
    fn rejects_add_file_over_existing_content() {
        let input = "\
*** Begin Patch
*** Add File: new.txt
+new
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert!(apply_diff("existing", patch).is_err());
    }

    #[test]
    fn applies_update_file_and_preserves_trailing_newline() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@
 context
-old
+new
 tail
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(
            apply_diff("start\ncontext\nold\ntail\nend\n", patch).unwrap(),
            "start\ncontext\nnew\ntail\nend\n"
        );
    }

    #[test]
    fn applies_update_without_trimming_surrounding_blank_lines() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@
-old
+new
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(apply_diff("\nold\n\n", patch).unwrap(), "new\n");
    }

    #[test]
    fn applies_update_with_named_hunk_header() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@ function_name
-old
+new
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(apply_diff("old", patch).unwrap(), "new\n");
    }

    #[test]
    fn applies_insertion_before_context() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@
+new
 existing
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(apply_diff("existing\n", patch).unwrap(), "new\nexisting\n");
    }

    #[test]
    fn applies_move_file_content_changes() {
        let input = "\
*** Begin Patch
*** Update File: old.txt
*** Move to: moved.txt
@@
-before
+after
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(apply_diff("before", patch).unwrap(), "after\n");
    }

    #[test]
    fn applies_move_file_without_content_changes() {
        let input = "\
*** Begin Patch
*** Update File: old.txt
*** Move to: moved.txt
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert_eq!(apply_diff("same\n", patch).unwrap(), "same\n");
    }

    #[test]
    fn rejects_update_when_hunk_does_not_match_base() {
        let input = "\
*** Begin Patch
*** Update File: changed.txt
@@
-old
+new
*** End Patch";
        let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

        assert!(apply_diff("other", patch).is_err());
    }
}
