use std::path::{Path, PathBuf};

use serde_json::Value;

use hapir_shared::rpc::skills::{RpcListSkillsRequest, RpcListSkillsResponse, RpcSkillSummary};

use crate::rpc::RpcRegistry;
use crate::utils::plugin::get_claude_installed_plugins;

fn codex_skills_root() -> PathBuf {
    let codex_home = std::env::var("CODEX_HOME")
        .ok()
        .or_else(|| dirs_next::home_dir().map(|h| h.join(".codex").to_string_lossy().to_string()))
        .unwrap_or_default();
    PathBuf::from(codex_home).join("skills")
}

fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    if !content.starts_with("---") {
        return (None, None);
    }
    let after_first = &content[3..];
    let newline_pos = after_first.find('\n').unwrap_or(0);
    let rest = &after_first[newline_pos + 1..];
    let Some(end) = rest.find("\n---") else {
        return (None, None);
    };
    let yaml_block = &rest[..end];

    let mut name = None;
    let mut description = None;
    for line in yaml_block.lines() {
        let trimmed = line.trim();
        if let Some(v) = trimmed.strip_prefix("name:") {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                name = Some(v);
            }
        }
        if let Some(v) = trimmed.strip_prefix("description:") {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                description = Some(v);
            }
        }
    }
    (name, description)
}

fn extract_skill_summary(skill_dir: &Path, content: &str) -> Option<RpcSkillSummary> {
    let (name_from_fm, description) = parse_frontmatter(content);
    let name_from_dir = skill_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let name = name_from_fm.unwrap_or(name_from_dir);
    if name.is_empty() {
        return None;
    }
    Some(RpcSkillSummary { name, description })
}

async fn list_skill_dirs(root: &Path) -> Vec<PathBuf> {
    let mut reader = match tokio::fs::read_dir(root).await {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let mut result = vec![];
    while let Ok(Some(entry)) = reader.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".system" {
            if let Ok(mut sub) = tokio::fs::read_dir(&path).await {
                while let Ok(Some(sub_entry)) = sub.next_entry().await {
                    if sub_entry.path().is_dir() {
                        result.push(sub_entry.path());
                    }
                }
            }
        } else {
            result.push(path);
        }
    }
    result
}

async fn scan_skills_from_dirs(dirs: &[PathBuf]) -> Vec<RpcSkillSummary> {
    let mut skills = vec![];
    for dir in dirs {
        let skill_file = dir.join("SKILL.md");
        if let Ok(content) = tokio::fs::read_to_string(&skill_file).await
            && let Some(summary) = extract_skill_summary(dir, &content)
        {
            skills.push(summary);
        }
    }
    skills
}

async fn list_skills_impl(agent: &str) -> Vec<RpcSkillSummary> {
    let mut skills = vec![];

    match agent {
        "codex" => {
            let root = codex_skills_root();
            let dirs = list_skill_dirs(&root).await;
            skills = scan_skills_from_dirs(&dirs).await;
        }
        "claude" => {
            let plugins = get_claude_installed_plugins().await;
            for plugin in &plugins {
                let skills_dir = plugin.install_path.join("skills");
                let plugin_skill_dirs = list_skill_dirs(&skills_dir).await;
                let plugin_skills = scan_skills_from_dirs(&plugin_skill_dirs).await;
                skills.extend(plugin_skills);
            }
        }
        _ => {}
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

pub async fn register_skills_handlers(rpc: &(impl RpcRegistry + Sync), _working_directory: &str) {
    rpc.register_rpc("listSkills", move |params: Value| async move {
        let mut response = RpcListSkillsResponse::default();
        let req: RpcListSkillsRequest = serde_json::from_value(params).unwrap_or_default();
        let skills = list_skills_impl(&req.agent).await;
        response.skills = Some(skills);
        response.success = true;
        serde_json::to_value(response).unwrap()
    })
    .await;
}
