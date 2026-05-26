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
        let mut lines = input.lines();
        let header = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invalid format for diff"))?;

        if header == "*** Begin Patch" {
            let mut vec = Vec::new();
            while let Some(patch) = Self::next_patch(&mut lines)? {
                vec.push(patch);
            }
            Ok(DiffSet { vec })
        } else {
            Err(anyhow::anyhow!("Invalid format for diff"))
        }
    }

    fn next_patch<'a>(lines: &mut std::str::Lines<'a>) -> anyhow::Result<Option<Patch<'a>>> {
        let (op, param) = match lines.next().map(|t| {
            let rest = t
                .strip_prefix("*** ")
                .ok_or_else(|| anyhow::anyhow!("Invalid format for diff"))?;

            rest.split_once(":")
                .map(|(a, b)| (a, b.trim()))
                .ok_or_else(|| anyhow::anyhow!("Invalid format for diff"))
        }) {
            Some(res) => res,
            None => return Ok(None),
        }?;

        let a_pair = lines
            .next()
            .and_then(|t1| {
                match t1.strip_prefix("*** ") {
                    // Move to op
                    Some(rest) => Some(
                        rest.split_once(":")
                            .map(|(a, b)| (a, b.trim()))
                            .ok_or_else(|| anyhow::anyhow!("Invalid format for diff")),
                    ),
                    None => None,
                }
            })
            .transpose()?;

        let prefix = PatchPrefix::new(op, param, a_pair)?;

        let res = match prefix.p_type {
            PatchType::AddFile => {
                let diff: anyhow::Result<Vec<_>> =
                    lines
                        .take_while(|line| !line.starts_with("***"))
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
                    Err(anyhow::anyhow!("Invalid format for diff"))
                }
            }
            PatchType::MoveFile => {
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
                    Err(anyhow::anyhow!("Invalid format for diff"))
                }
            }
        }?;

        Ok(Some(res))
    }

    fn get_patch_changes<'a>(lines: &mut std::str::Lines<'a>) -> anyhow::Result<PatchChange<'a>> {
        let (neg, pos, search) = lines.take_while(|line| !line.starts_with("***")).try_fold(
            (Vec::new(), Vec::new(), None),
            |(mut neg, mut pos, search), line| match line.chars().next().unwrap() {
                ' ' => Ok((neg, pos, Some(line))),
                '-' => {
                    neg.push(line);
                    Ok((neg, pos, search))
                }
                '+' => {
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
