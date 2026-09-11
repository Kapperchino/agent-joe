use super::*;

#[test]
fn relative_path_uses_path_components() {
    let root = std::env::temp_dir().join("project");
    let path = root.join("src/lib.rs");
    let relative = RPath::new(path, root.to_string_lossy().into_owned()).unwrap();
    assert_eq!(
        relative.inner,
        PathBuf::from("src/lib.rs").to_str().unwrap()
    );
}

#[test]
fn outside_path_error_identifies_the_file_and_root() {
    let directory = std::env::temp_dir();
    let root = directory.join("project");
    let path = directory.join("project-dependency/src/lib.rs");
    let error = RPath::new(path.clone(), root.to_string_lossy().into_owned()).unwrap_err();
    let message = error.to_string();
    assert!(message.contains(&path.display().to_string()));
    assert!(message.contains(&root.display().to_string()));
    assert!(message.contains("outside project root"));
}
