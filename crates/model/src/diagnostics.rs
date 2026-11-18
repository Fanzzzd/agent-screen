//! Build comparison is evidence about source metadata, not binary equality.

#[derive(Clone)]
pub struct BuildEvidence<'a> {
    pub package_version: &'a str,
    pub revision: Option<&'a str>,
    pub dirty: Option<bool>,
    pub profile: Option<&'a str>,
    pub target: Option<&'a str>,
    pub protocol_version: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Comparison {
    SameSource,
    Different,
    Unknown,
}

pub fn compare(a: &BuildEvidence<'_>, b: &BuildEvidence<'_>) -> Comparison {
    if a.package_version != b.package_version
        || a.protocol_version != b.protocol_version
        || matches!((a.revision, b.revision), (Some(x), Some(y)) if x != y)
        || matches!((a.profile, b.profile), (Some(x), Some(y)) if x != y)
        || matches!((a.target, b.target), (Some(x), Some(y)) if x != y)
    {
        return Comparison::Different;
    }
    if a.package_version.is_empty()
        || a.revision.is_none()
        || b.revision.is_none()
        || a.dirty != Some(false)
        || b.dirty != Some(false)
        || a.profile.is_none()
        || b.profile.is_none()
        || a.target.is_none()
        || b.target.is_none()
    {
        return Comparison::Unknown;
    }
    Comparison::SameSource
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_clean_metadata_can_match() {
        let clean = BuildEvidence {
            package_version: "0.1.17",
            revision: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            dirty: Some(false),
            profile: Some("release"),
            target: Some("aarch64-apple-darwin"),
            protocol_version: 1,
        };
        assert_eq!(compare(&clean, &clean), Comparison::SameSource);
        let mut other = clean.clone();
        other.revision = Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(compare(&clean, &other), Comparison::Different);
        other = clean.clone();
        other.dirty = Some(true);
        assert_eq!(compare(&other, &other), Comparison::Unknown);
        other = clean.clone();
        other.revision = None;
        assert_eq!(compare(&clean, &other), Comparison::Unknown);
        other = clean.clone();
        other.profile = None;
        assert_eq!(compare(&clean, &other), Comparison::Unknown);
        other = clean.clone();
        other.package_version = "0.1.16";
        assert_eq!(compare(&clean, &other), Comparison::Different);
        other = clean.clone();
        other.protocol_version = 2;
        assert_eq!(compare(&clean, &other), Comparison::Different);
    }
}
