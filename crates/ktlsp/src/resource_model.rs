//! Captured Gradle resource selections. Missing claims never authorize directory discovery.
use std::path::{Component, Path, PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourcePatterns {
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTree {
    pub root: PathBuf,
    /// The intersection of the actual stock Gradle pattern sets for this tree.
    pub patterns: Vec<ResourcePatterns>,
    pub future_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceFile {
    pub path: PathBuf,
    /// Actual Gradle FileVisitDetails relative path, not a filename-based guess.
    pub relative_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceClaims {
    pub adapter: String,
    pub roots: Vec<PathBuf>,
    pub trees: Vec<ResourceTree>,
    pub files: Vec<ResourceFile>,
    pub output_roots: Vec<PathBuf>,
    pub configuration_inputs: Vec<PathBuf>,
    pub default_excludes: Vec<String>,
    pub complete: bool,
    pub problem: Option<String>,
}

pub fn safe_relative(path: &str) -> bool {
    !path.is_empty() && path.len() <= 16 * 1024 && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && Path::new(path).components().all(|part| matches!(part, Component::Normal(_)))
        && !path.split('/').any(|part| part.is_empty() || matches!(part, "." | "..") || part.contains(':'))
}

impl ResourceClaims {
    pub fn valid(&self) -> bool {
        self.adapter == "gradle-source-set-resources-v1"
            && self.roots.len() <= 1024 && self.trees.len() <= 1024
            && self.files.len() <= 1_000_000 && self.configuration_inputs.len() <= 1024 && self.output_roots.len() <= 1024
            && self.roots.iter().chain(&self.output_roots).chain(&self.configuration_inputs)
                .all(|path| path.is_absolute() && !path.components().any(|part| matches!(part, Component::ParentDir)))
            && self.files.iter().all(|file| file.path.is_absolute()
                && !file.path.components().any(|part| matches!(part,Component::ParentDir)) && safe_relative(&file.relative_path)
                && self.roots.iter().any(|root| file.path.starts_with(root)))
            && self.trees.iter().all(|tree| self.roots.contains(&tree.root)
                && tree.patterns.len() <= 32 && tree.patterns.iter().all(|set|
                    set.includes.len() <= 1024 && set.excludes.len() <= 1024
                    && set.includes.iter().chain(&set.excludes).all(|pattern| valid_pattern(pattern))))
            && self.default_excludes.len() <= 256 && self.default_excludes.iter().all(|pattern| valid_pattern(pattern))
    }

    /// These immutable claims must be validated once at the model/read boundary.
    pub(crate) fn selects(&self, tree: &ResourceTree, relative: &str) -> Option<bool> {
        if !tree.future_complete || !safe_relative(relative) { return None; }
        Some(!self.default_excludes.iter().any(|pattern| ant_match(pattern, relative))
            && tree.patterns.iter().all(|set|
                (set.includes.is_empty() || set.includes.iter().any(|pattern| ant_match(pattern,relative)))
                && !set.excludes.iter().any(|pattern| ant_match(pattern,relative))))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedResourceContext {
    pub scope: ktcore::project_model::ProjectScope,
    pub origin: ktcore::project_model::FileOrigin,
    pub path: PathBuf,
    pub relative_path: String,
    pub association: &'static str,
}

fn valid_pattern(pattern: &str) -> bool {
    !pattern.is_empty() && pattern.len() <= 4096 && !pattern.contains(['\\', ':'])
        && !pattern.chars().any(char::is_control)
        && !pattern.starts_with('/') && !pattern.split('/').any(|part| matches!(part,"."|".."))
}

/// Ant's only wildcards are *, ? and the whole ** segment; brackets are literal.
fn ant_match(pattern: &str, relative: &str) -> bool {
    let pattern = if pattern.ends_with('/') { format!("{pattern}**") } else { pattern.into() };
    let patterns: Vec<_> = pattern.split('/').collect();
    let segments: Vec<_> = relative.split('/').collect();
    let mut previous=vec![false;segments.len()+1]; previous[0]=true;
    for part in patterns {
        let mut current=vec![false;segments.len()+1];
        if part=="**" {
            current[0]=previous[0];
            for index in 1..=segments.len() { current[index]=previous[index] || current[index-1]; }
        } else {
            for index in 1..=segments.len() { current[index]=previous[index-1] && segment_match(part,segments[index-1]); }
        }
        previous=current;
    }
    previous[segments.len()]
}

fn segment_match(pattern: &str, segment: &str) -> bool {
    // Gradle/Ant compares Java chars (UTF-16 code units), not Unicode scalar values.
    let chars:Vec<_>=segment.encode_utf16().collect();
    let mut previous=vec![false;chars.len()+1]; previous[0]=true;
    for character in pattern.encode_utf16() {
        let mut current=vec![false;chars.len()+1];
        if character==b'*' as u16 { current[0]=previous[0]; }
        for index in 1..=chars.len() {
            current[index]=if character==b'*' as u16 { previous[index] || current[index-1] }
                else { previous[index-1] && (character==b'?' as u16 || character==chars[index-1]) };
        }
        previous=current;
    }
    previous[chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_patterns_preserve_ant_intersections_and_safe_relative_names() {
        for (pattern,path,expected) in [
            ("**/*.json","config.json",true),("**/*.json","nested/config.json",true),
            ("META-INF/**","META-INF/accesstransformer.cfg",true),("META-INF/**","other/META-INF/a.cfg",false),
            ("*.json","nested/config.json",false),("data/","data/deep/a.json",true),
            ("a?.json","a😀.json",false),("a??.json","a😀.json",true),("[a].json","a.json",false),("[a].json","[a].json",true),
        ] { assert_eq!(ant_match(pattern,path),expected,"{pattern} {path}"); }
        for path in ["../outside.json","/outside.json","a/../outside.json","C:/outside.json","a\\b","a//b",""] {
            assert!(!safe_relative(path),"{path}");
        }
        assert!(safe_relative("META-INF/😀.json"));
    }
}
