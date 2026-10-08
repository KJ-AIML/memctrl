//! Deterministic hierarchical keyword clustering, with Unicode-safe summaries.
use crate::model::{Memory, TreeNode};
use crate::sanitize::sanitize_text;
use std::collections::BTreeMap;

const GROUPS: &[(&str, &[&str])] = &[
    (
        "tech_stack",
        &[
            "use",
            "using",
            "framework",
            "library",
            "database",
            "backend",
            "frontend",
            "api",
            "service",
        ],
    ),
    (
        "decisions",
        &[
            "decided",
            "adr",
            "choice",
            "chose",
            "migrate",
            "switch",
            "architecture",
        ],
    ),
    (
        "preferences",
        &[
            "prefer",
            "like",
            "style",
            "pattern",
            "convention",
            "always",
            "never",
        ],
    ),
    (
        "tasks",
        &[
            "implement",
            "building",
            "working",
            "task",
            "wip",
            "feature",
            "fix",
        ],
    ),
    ("team", &["team", "meeting", "review", "standup", "sprint"]),
    ("other", &[]),
];

fn title(value: &str) -> String {
    value
        .split('_')
        .map(|w| {
            let mut chars = w.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn layer_node(layer: &str, memories: &[&Memory]) -> TreeNode {
    let mut children = Vec::new();
    for (name, _) in GROUPS {
        let members: Vec<_> = memories
            .iter()
            .filter(|m| {
                let content = m.content.to_lowercase();
                GROUPS
                    .iter()
                    .find(|(_, words)| words.iter().any(|word| content.contains(word)))
                    .map(|(n, _)| n)
                    .unwrap_or(&"other")
                    == name
            })
            .collect();
        if members.is_empty() {
            continue;
        }
        children.push(TreeNode {
            id: format!("cluster_{layer}_{name}"),
            title: title(name),
            layer: Some(layer.into()),
            summary: format!("{} memories about {name}", members.len()),
            memory_ids: members.iter().map(|m| m.id.clone()).collect(),
            children: members
                .iter()
                .map(|m| {
                    let content = sanitize_text(&m.content);
                    TreeNode {
                        id: format!("mem_{}", m.id),
                        title: content.chars().take(60).collect(),
                        summary: content,
                        layer: Some(layer.into()),
                        memory_ids: vec![m.id.clone()],
                        children: Vec::new(),
                    }
                })
                .collect(),
        });
    }
    TreeNode {
        id: format!("layer_{layer}"),
        title: title(layer),
        summary: format!("{} memories in layer '{layer}'", memories.len()),
        layer: Some(layer.into()),
        memory_ids: memories.iter().map(|m| m.id.clone()).collect(),
        children,
    }
}

pub fn build_tree(memories: &[Memory]) -> TreeNode {
    let mut layers: BTreeMap<&str, Vec<&Memory>> = BTreeMap::new();
    for memory in memories {
        layers.entry(&memory.layer).or_default().push(memory);
    }
    // IDs, rather than insertion order, provide reproducible tie breaking and trees.
    for memories in layers.values_mut() {
        memories.sort_by(|a, b| a.id.cmp(&b.id));
    }
    TreeNode {
        id: "root".into(),
        title: "Memory Tree".into(),
        layer: Some("root".into()),
        memory_ids: Vec::new(),
        summary: if memories.is_empty() {
            "Empty memory store".into()
        } else {
            format!(
                "Root node with {} layers, {} total memories",
                layers.len(),
                memories.len()
            )
        },
        children: layers
            .into_iter()
            .map(|(layer, memories)| layer_node(layer, &memories))
            .collect(),
    }
}

pub fn incremental_tree(previous: &TreeNode, memories: &[Memory]) -> TreeNode {
    fn leaf_contents(node: &TreeNode, contents: &mut BTreeMap<String, String>) {
        if node.children.is_empty() && node.memory_ids.len() == 1 {
            contents.insert(node.memory_ids[0].clone(), node.summary.clone());
        }
        for child in &node.children {
            leaf_contents(child, contents);
        }
    }
    let mut layers: BTreeMap<&str, Vec<&Memory>> = BTreeMap::new();
    for memory in memories {
        layers.entry(&memory.layer).or_default().push(memory);
    }
    let mut children = Vec::new();
    for (layer, mut members) in layers {
        members.sort_by(|a, b| a.id.cmp(&b.id));
        let expected: BTreeMap<_, _> = members
            .iter()
            .map(|m| (m.id.clone(), sanitize_text(&m.content)))
            .collect();
        let reusable = previous.children.iter().find(|node| {
            if node.layer.as_deref() != Some(layer) {
                return false;
            }
            let mut old = BTreeMap::new();
            leaf_contents(node, &mut old);
            old == expected
        });
        children.push(
            reusable
                .cloned()
                .unwrap_or_else(|| layer_node(layer, &members)),
        );
    }
    TreeNode {
        id: "root".into(),
        title: "Memory Tree".into(),
        layer: Some("root".into()),
        memory_ids: Vec::new(),
        summary: if memories.is_empty() {
            "Empty memory store".into()
        } else {
            format!(
                "Root node with {} layers, {} total memories",
                children.len(),
                memories.len()
            )
        },
        children,
    }
}
