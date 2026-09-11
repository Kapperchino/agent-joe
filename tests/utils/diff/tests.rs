use super::*;

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
fn applies_named_and_unnamed_hunks_preserving_surrounding_content() {
    let input = "\
*** Begin Patch
*** Update File: src/main.rs
@@ first
-old first
+new first
@@
-old second
+new second
@@ third
-old third
+new third
*** End Patch";
    let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

    assert_eq!(
        apply_diff(
            "start\nold first\nbetween\nold second\nold third\nend\n",
            patch
        )
        .unwrap(),
        "start\nnew first\nbetween\nnew second\nnew third\nend\n"
    );
}

#[test]
fn applies_multiple_hunks_to_repeated_anchors_in_source_order() {
    let input = "\
*** Begin Patch
*** Update File: changed.txt
@@
+first
 marker
@@
+second
 marker
*** End Patch";
    let patch = DiffSet::new(input).unwrap().vec.into_iter().next().unwrap();

    assert_eq!(
        apply_diff("marker\nmarker\n", patch).unwrap(),
        "first\nmarker\nsecond\nmarker\n"
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
