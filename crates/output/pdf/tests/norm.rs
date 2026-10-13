#[test]
fn norm_sees_differences() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures/pdf");
    let a = std::fs::read(dir.join("hello.pdf")).unwrap();
    let b = std::fs::read(dir.join("mixed1.pdf")).unwrap();
    let c = std::fs::read(dir.join("compressed.pdf")).unwrap();
    let na = phitex_pdf::norm::normal_form(&a).unwrap();
    eprintln!("{} lines; first: {:?}", na.len(), &na[..na.len().min(3)]);
    assert!(phitex_pdf::norm::first_difference(&a, &b).is_some());
    eprintln!(
        "hello vs compressed: {:?}",
        phitex_pdf::norm::first_difference(&a, &c)
    );
    assert!(phitex_pdf::norm::first_difference(&a, &a).is_none());
}
