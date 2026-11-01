use crate::reader::{EventReader, XmlEvent};

#[test]
fn toss_namespace_snapshots_are_opt_in_but_qnames_stay_resolved() {
    let source = r#"<w:root xmlns:w="urn:word" xmlns:a="urn:attribute" a:key="value"><w:child xmlns:w="urn:nested"/></w:root>"#;
    let starts: Vec<_> = EventReader::from_str(source)
        .into_iter()
        .filter_map(|event| match event.unwrap() {
            XmlEvent::StartElement {
                name,
                attributes,
                namespace,
            } => Some((name, attributes, namespace)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0].0.namespace.as_deref(), Some("urn:word"));
    assert_eq!(
        starts[0].1[0].name.namespace.as_deref(),
        Some("urn:attribute")
    );
    assert_eq!(starts[1].0.namespace.as_deref(), Some("urn:nested"));
    if cfg!(feature = "toss-omit-namespace-maps") {
        assert!(starts.iter().all(|entry| entry.2 .0.is_empty()));
    } else {
        assert_eq!(starts[0].2.get("w"), Some("urn:word"));
        assert_eq!(starts[1].2.get("w"), Some("urn:nested"));
    }
}

#[test]
fn toss_prefix_shadowing_restores_parent_and_default_namespaces() {
    let source = r#"<root xmlns="urn:root" xmlns:p="urn:outer"><p:inner xmlns:p="urn:inner"/><p:outer/><plain xmlns=""/></root>"#;
    let names: Vec<_> = EventReader::from_str(source)
        .into_iter()
        .filter_map(|event| match event.unwrap() {
            XmlEvent::StartElement { name, .. } => Some((name.local_name, name.namespace)),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("root".into(), Some("urn:root".into())),
            ("inner".into(), Some("urn:inner".into())),
            ("outer".into(), Some("urn:outer".into())),
            ("plain".into(), None)
        ]
    );
}

#[test]
fn toss_unbound_prefixes_and_mismatched_qnames_still_fail() {
    for source in [
        "<missing:root/>",
        "<root missing:attr='x'/>",
        "<p:root xmlns:p='urn:p'></root>",
        "<root xmlns:xml='urn:wrong'/>",
        "<root><broken></root>",
    ] {
        assert!(
            EventReader::from_str(source)
                .into_iter()
                .any(|event| event.is_err()),
            "accepted malformed namespace/XML: {source}"
        );
    }
}

#[test]
fn toss_text_entities_cdata_and_unicode_are_unchanged() {
    let source = "<root xmlns='urn:x'>中文🙂 &amp; &lt; &#x3b1;<![CDATA[<>]]></root>";
    let text: String = EventReader::from_str(source)
        .into_iter()
        .filter_map(|event| match event.unwrap() {
            XmlEvent::Characters(text) | XmlEvent::CData(text) => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(text, "中文🙂 & < α<>");
}
