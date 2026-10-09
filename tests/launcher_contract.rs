use rclaude::launcher::{LaunchError, engine_requirement, private_engine_copy};
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn the_engine_must_have_the_pinned_certificate_and_exact_identifier() {
    let requirement = engine_requirement(&"A".repeat(40)).unwrap();
    assert!(requirement.contains("io.github.typiqally.rclaude.engine"));
    assert!(requirement.contains(&format!("certificate leaf = H\"{}\"", "A".repeat(40))));
    assert_eq!(engine_requirement("\" or true").unwrap_err(), LaunchError::InvalidIdentity);
}

#[test]
fn the_loader_uses_a_private_copy_and_refuses_unsafe_sources() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("engine.dylib");
    std::fs::write(&source, b"synthetic engine").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    let copy = private_engine_copy(&source).unwrap();
    std::fs::write(&source, b"replaced engine").unwrap();
    assert_eq!(std::fs::read(copy.path()).unwrap(), b"synthetic engine");
    assert_eq!(std::fs::metadata(copy.path()).unwrap().permissions().mode() & 0o077, 0);
    assert_eq!(std::fs::metadata(copy.path().parent().unwrap()).unwrap().permissions().mode() & 0o077, 0);
    let link = directory.path().join("linked.dylib");
    symlink(&source, &link).unwrap();
    assert!(private_engine_copy(&link).is_err());
    let hard = directory.path().join("hard.dylib");
    std::fs::hard_link(&source, &hard).unwrap();
    assert!(private_engine_copy(&source).is_err());
    std::fs::remove_file(hard).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(private_engine_copy(&source).is_err());
}
