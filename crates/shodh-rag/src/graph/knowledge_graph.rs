use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub name: String,
    pub entity_type: String,
    pub doc_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub relation_type: String,
    pub weight: f32,
}

pub struct KnowledgeGraph {
    graph: DiGraph<Entity, Relationship>,
    name_to_node: HashMap<String, NodeIndex>,
    max_nodes: usize,
}

impl KnowledgeGraph {
    pub fn new(max_nodes: usize) -> Self {
        Self {
            graph: DiGraph::new(),
            name_to_node: HashMap::new(),
            max_nodes,
        }
    }

    pub fn add_entity(&mut self, name: &str, entity_type: &str, doc_id: &str) -> NodeIndex {
        if let Some(&idx) = self.name_to_node.get(name) {
            if let Some(entity) = self.graph.node_weight_mut(idx) {
                if !entity.doc_ids.contains(&doc_id.to_string()) {
                    entity.doc_ids.push(doc_id.to_string());
                }
            }
            return idx;
        }

        if self.graph.node_count() >= self.max_nodes {
            return self
                .name_to_node
                .values()
                .next()
                .copied()
                .unwrap_or(NodeIndex::new(0));
        }

        let entity = Entity {
            name: name.to_string(),
            entity_type: entity_type.to_string(),
            doc_ids: vec![doc_id.to_string()],
        };

        let idx = self.graph.add_node(entity);
        self.name_to_node.insert(name.to_string(), idx);
        idx
    }

    pub fn add_relationship(&mut self, from: &str, to: &str, relation_type: &str, weight: f32) {
        let from_idx = if let Some(&idx) = self.name_to_node.get(from) {
            idx
        } else {
            return;
        };
        let to_idx = if let Some(&idx) = self.name_to_node.get(to) {
            idx
        } else {
            return;
        };

        self.graph.add_edge(
            from_idx,
            to_idx,
            Relationship {
                relation_type: relation_type.to_string(),
                weight,
            },
        );
    }

    pub fn get_related_doc_ids(&self, entity_name: &str, max_hops: usize) -> Vec<String> {
        let Some(&start) = self.name_to_node.get(entity_name) else {
            return Vec::new();
        };

        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        let mut doc_ids = Vec::new();

        queue.push_back((start, 0));
        visited.insert(start);

        while let Some((node, depth)) = queue.pop_front() {
            if let Some(entity) = self.graph.node_weight(node) {
                for doc_id in &entity.doc_ids {
                    if !doc_ids.contains(doc_id) {
                        doc_ids.push(doc_id.clone());
                    }
                }
            }

            if depth < max_hops {
                for neighbor in self.graph.neighbors(node) {
                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        queue.push_back((neighbor, depth + 1));
                    }
                }
            }
        }

        doc_ids
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    pub fn clear(&mut self) {
        self.graph.clear();
        self.name_to_node.clear();
    }
}
