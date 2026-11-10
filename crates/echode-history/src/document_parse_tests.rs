use super::*;

fn header() -> Header {
    Header {
        metadata: ProjectMetadata::from(&Project::new()),
        tempo: Project::new().tempo().clone(),
        content_ids: vec![Id::from_raw(1).to_string()],
        track_ids: vec![Id::from_raw(2).to_string()],
    }
}

#[test]
fn assembly_checks_file_counts_and_order_before_decoding() {
    let content = || vec![(format!("{}.json", Id::from_raw(1)), String::new())];
    for (contents, tracks, expected) in [
        (content(), vec![], PackageKind::Track),
        (
            vec![],
            vec![(String::new(), String::new())],
            PackageKind::Content,
        ),
    ] {
        let error = assemble(header(), contents, tracks, Path::new("unused")).unwrap_err();
        assert!(
            matches!(error, PackageError::Validation(Invalid::FileCountMismatch(kind)) if kind == expected)
        );
        assert_eq!(
            error.to_string(),
            format!("{expected} list does not match the {expected} files")
        );
    }
    let filename = "wrong.json";
    let error = assemble(
        header(),
        vec![(filename.into(), String::new())],
        vec![(String::new(), String::new())],
        Path::new("unused"),
    )
    .unwrap_err();
    assert!(
        matches!(&error, PackageError::Validation(Invalid::FileOutOfOrder {
        kind: PackageKind::Content, filename: found,
    }) if found == filename)
    );
    assert_eq!(error.to_string(), "contents/wrong.json is out of order");
    let error =
        check_filename(&Id::from_raw(2).to_string(), filename, PackageKind::Track).unwrap_err();
    assert!(matches!(
        error,
        Invalid::FileOutOfOrder {
            kind: PackageKind::Track,
            ..
        }
    ));
    assert_eq!(error.to_string(), "tracks/wrong.json is out of order");
}

#[test]
fn ids_are_canonicalized_before_becoming_paths() {
    let canonical = Id::from_raw(0xab).to_string();
    assert_eq!(
        canonical_ids(vec![canonical.to_uppercase()]).unwrap(),
        vec![canonical]
    );
    assert!(matches!(parse_id("../outside"), Err(Invalid::InvalidId(id)) if id == "../outside"));
    assert!(
        matches!(canonical_ids(vec!["../outside".into()]), Err(Invalid::InvalidListedId(id)) if id == "../outside")
    );
}
