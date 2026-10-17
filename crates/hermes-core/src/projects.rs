//! Project registry types and local-folder validation, independent of the UI.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Folder {
    pub path: String,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub primary_path: Option<String>,
    pub folders: Vec<Folder>,
    pub archived: bool,
}

impl Project {
    pub fn root(&self) -> &str {
        self.primary_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .or_else(|| self.folders.iter().find(|f| f.is_primary).map(|f| f.path.as_str()))
            .or_else(|| self.folders.first().map(|f| f.path.as_str()))
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProjectList {
    pub projects: Vec<Project>,
    pub active_id: Option<String>,
}

/// Match complete path components, preferring the most specific registered folder.
pub fn for_cwd<'a>(projects: &'a [Project], cwd: &str) -> Option<&'a Project> {
    let cwd = Path::new(cwd);
    if !cwd.is_absolute() {
        return None;
    }
    projects
        .iter()
        .filter(|p| !p.archived)
        .filter_map(|project| {
            std::iter::once(project.root())
                .chain(project.folders.iter().map(|f| f.path.as_str()))
                .filter(|root| !root.is_empty() && cwd.starts_with(Path::new(root)))
                .map(|root| Path::new(root).components().count())
                .max()
                .map(|depth| (project, depth))
        })
        .max_by_key(|(_, depth)| *depth)
        .map(|(project, _)| project)
}

/// Do not silently make a mistyped folder or expand shell syntax.
pub fn existing_folder(input: &str, home: &Path) -> Result<PathBuf, String> {
    let input = input.trim();
    let path = if input == "~" {
        home.to_owned()
    } else if let Some(tail) = input.strip_prefix("~/") {
        home.join(tail)
    } else {
        PathBuf::from(input)
    };
    if !path.is_absolute() {
        return Err("Enter the full path to an existing folder.".into());
    }
    if !path.is_dir() {
        return Err("That folder does not exist or cannot be opened.".into());
    }
    path.canonicalize().map_err(|e| format!("Could not open folder: {e}"))
}

pub const REASONING_LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn root_uses_primary_then_primary_folder_then_first() {
        let mut p: Project =
            serde_json::from_value(json!({"folders":[{"path":"/one"},{"path":"/two","is_primary":true}]})).unwrap();
        assert_eq!(p.root(), "/two");
        p.primary_path = Some("/explicit".into());
        assert_eq!(p.root(), "/explicit");
        p.primary_path = None;
        p.folders[1].is_primary = false;
        assert_eq!(p.root(), "/one");
    }

    #[test]
    fn restores_project_from_sparse_info_cwd_without_prefix_confusion() {
        let projects: Vec<Project> = serde_json::from_value(json!([
            {"id":"one", "primary_path":"/work/app"},
            {"id":"nested", "folders":[{"path":"/work/app/nested"}]}
        ]))
        .unwrap();
        assert_eq!(for_cwd(&projects, "/work/app/src").unwrap().id, "one");
        assert_eq!(for_cwd(&projects, "/work/app/nested/src").unwrap().id, "nested");
        assert!(for_cwd(&projects, "/work/application").is_none());
        assert!(for_cwd(&projects, "").is_none());
    }

    #[test]
    fn rejects_missing_and_relative_folders_without_creating_them() {
        let home = std::env::temp_dir();
        assert!(existing_folder("relative/path", &home).is_err());
        let missing = home.join(format!("missing-project-{}", std::process::id()));
        assert!(existing_folder(&missing.to_string_lossy(), &home).is_err());
        assert!(!missing.exists());
        assert_eq!(existing_folder("~", &home).unwrap(), home.canonicalize().unwrap());
    }
}
