pub mod cache;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use zerho_core::{ContentHash, ModuleId, ModuleKind};

pub use cache::{CacheStats, CachedArtifact, DiskCache};

/// A single node in the incremental module graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleNode {
    pub id: ModuleId,
    pub path: PathBuf,
    pub kind: ModuleKind,
    pub hash: ContentHash,
    pub imports: BTreeSet<ModuleId>,
    pub importers: BTreeSet<ModuleId>,
}

/// Summary metrics exposed to `/__inspect` and CLI diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphStats {
    pub module_count: usize,
    pub edge_count: usize,
}

/// Incremental module dependency graph with reverse-edge tracking for instant HMR invalidation.
#[derive(Debug, Default)]
pub struct ModuleGraph {
    nodes: HashMap<ModuleId, ModuleNode>,
}

impl ModuleGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or updates a module and synchronizes forward/reverse dependency edges.
    /// Returns `true` if the module is new or its content hash changed.
    pub fn upsert_module(
        &mut self,
        id: ModuleId,
        path: PathBuf,
        kind: ModuleKind,
        hash: ContentHash,
        new_imports: impl IntoIterator<Item = ModuleId>,
    ) -> bool {
        let new_imports: BTreeSet<ModuleId> = new_imports.into_iter().collect();

        let (changed, old_imports) = match self.nodes.get(&id) {
            Some(existing) => (existing.hash != hash || existing.imports != new_imports, existing.imports.clone()),
            None => (true, BTreeSet::new()),
        };

        // Remove stale reverse edges.
        for removed in old_imports.difference(&new_imports) {
            if let Some(target) = self.nodes.get_mut(removed) {
                target.importers.remove(&id);
            }
        }

        // Preserve existing reverse edges for this node.
        let existing_importers = self
            .nodes
            .get(&id)
            .map(|n| n.importers.clone())
            .unwrap_or_default();

        self.nodes.insert(
            id,
            ModuleNode {
                id,
                path,
                kind,
                hash,
                imports: new_imports.clone(),
                importers: existing_importers,
            },
        );

        // Register new reverse edges on already-known targets.
        for imported in &new_imports {
            if let Some(target) = self.nodes.get_mut(imported) {
                target.importers.insert(id);
            }
        }

        // Ensure any existing node that already imported `id` is linked in `id.importers`.
        let reverse_links: Vec<ModuleId> = self
            .nodes
            .values()
            .filter(|node| node.id != id && node.imports.contains(&id))
            .map(|node| node.id)
            .collect();
        if let Some(node) = self.nodes.get_mut(&id) {
            node.importers.extend(reverse_links);
        }

        changed
    }

    pub fn get(&self, id: ModuleId) -> Option<&ModuleNode> {
        self.nodes.get(&id)
    }

    /// Computes all modules transitively affected when `root` changes (BFS over reverse edges).
    pub fn invalidate(&self, root: ModuleId) -> Vec<ModuleId> {
        let mut affected = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        if self.nodes.contains_key(&root) {
            visited.insert(root);
            queue.push_back(root);
        }

        while let Some(current) = queue.pop_front() {
            affected.push(current);
            if let Some(node) = self.nodes.get(&current) {
                for &importer in &node.importers {
                    if visited.insert(importer) {
                        queue.push_back(importer);
                    }
                }
            }
        }

        affected
    }

    pub fn stats(&self) -> GraphStats {
        let edge_count = self.nodes.values().map(|n| n.imports.len()).sum();
        GraphStats { module_count: self.nodes.len(), edge_count }
    }

    /// Returns a deterministic, path-sorted snapshot of all modules in the graph for `/__inspect`.
    pub fn snapshot(&self) -> Vec<&ModuleNode> {
        let mut list: Vec<&ModuleNode> = self.nodes.values().collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        list
    }

    /// Detects all directed dependency cycles in the module graph (DFS 3-color traversal)
    /// and returns deterministic path chains (e.g. `[A, B, A]`).
    pub fn find_cycles(&self) -> Vec<Vec<PathBuf>> {
        // 0 = unvisited, 1 = visiting (on stack), 2 = done
        let mut state: HashMap<ModuleId, u8> = HashMap::new();
        let mut stack: Vec<ModuleId> = Vec::new();
        let mut seen_signatures: HashSet<Vec<PathBuf>> = HashSet::new();
        let mut cycles: Vec<Vec<PathBuf>> = Vec::new();

        let mut ordered_ids: Vec<ModuleId> = self.nodes.values().map(|n| n.id).collect();
        ordered_ids.sort_by(|a, b| {
            let pa = self.nodes.get(a).map(|n| &n.path);
            let pb = self.nodes.get(b).map(|n| &n.path);
            pa.cmp(&pb)
        });

        for start in ordered_ids {
            if state.get(&start).copied().unwrap_or(0) == 0 {
                self.dfs_cycles(start, &mut state, &mut stack, &mut seen_signatures, &mut cycles);
            }
        }
        cycles
    }

    fn dfs_cycles(
        &self,
        current: ModuleId,
        state: &mut HashMap<ModuleId, u8>,
        stack: &mut Vec<ModuleId>,
        seen_signatures: &mut HashSet<Vec<PathBuf>>,
        cycles: &mut Vec<Vec<PathBuf>>,
    ) {
        state.insert(current, 1);
        stack.push(current);

        if let Some(node) = self.nodes.get(&current) {
            for &dep in &node.imports {
                match state.get(&dep).copied().unwrap_or(0) {
                    0 => self.dfs_cycles(dep, state, stack, seen_signatures, cycles),
                    1 => {
                        if let Some(pos) = stack.iter().position(|&id| id == dep) {
                            let mut chain: Vec<PathBuf> = stack[pos..]
                                .iter()
                                .filter_map(|id| self.nodes.get(id).map(|n| n.path.clone()))
                                .collect();
                            if let Some(first) = chain.first().cloned() {
                                chain.push(first);
                            }
                            if seen_signatures.insert(chain.clone()) {
                                cycles.push(chain);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        stack.pop();
        state.insert(current, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitive_invalidation_propagates_to_importers() {
        let mut graph = ModuleGraph::new();
        let entry = ModuleId::from_specifier("src/main.tsx");
        let app = ModuleId::from_specifier("src/App.tsx");
        let btn = ModuleId::from_specifier("src/Button.tsx");

        graph.upsert_module(
            btn,
            PathBuf::from("src/Button.tsx"),
            ModuleKind::Tsx,
            ContentHash::from_bytes(b"btn-v1"),
            [],
        );
        graph.upsert_module(
            app,
            PathBuf::from("src/App.tsx"),
            ModuleKind::Tsx,
            ContentHash::from_bytes(b"app-v1"),
            [btn],
        );
        graph.upsert_module(
            entry,
            PathBuf::from("src/main.tsx"),
            ModuleKind::Tsx,
            ContentHash::from_bytes(b"main-v1"),
            [app],
        );

        let invalidated = graph.invalidate(btn);
        assert_eq!(invalidated, vec![btn, app, entry]);
        assert_eq!(graph.stats(), GraphStats { module_count: 3, edge_count: 2 });
    }

    #[test]
    fn removing_import_cleans_reverse_edge() {
        let mut graph = ModuleGraph::new();
        let app = ModuleId::from_specifier("src/App.tsx");
        let dep = ModuleId::from_specifier("src/dep.ts");

        graph.upsert_module(dep, PathBuf::from("src/dep.ts"), ModuleKind::Ts, ContentHash::from_bytes(b"1"), []);
        graph.upsert_module(app, PathBuf::from("src/App.tsx"), ModuleKind::Tsx, ContentHash::from_bytes(b"1"), [dep]);
        assert_eq!(graph.invalidate(dep), vec![dep, app]);

        // App stops importing dep.
        graph.upsert_module(app, PathBuf::from("src/App.tsx"), ModuleKind::Tsx, ContentHash::from_bytes(b"2"), []);
        assert_eq!(graph.invalidate(dep), vec![dep]);
    }

    #[test]
    fn detects_circular_dependency_cycles() {
        let mut graph = ModuleGraph::new();
        let a = ModuleId::from_specifier("src/a.ts");
        let b = ModuleId::from_specifier("src/b.ts");
        let c = ModuleId::from_specifier("src/c.ts");

        graph.upsert_module(a, PathBuf::from("src/a.ts"), ModuleKind::Ts, ContentHash::from_bytes(b"a"), [b]);
        graph.upsert_module(b, PathBuf::from("src/b.ts"), ModuleKind::Ts, ContentHash::from_bytes(b"b"), [c]);
        graph.upsert_module(c, PathBuf::from("src/c.ts"), ModuleKind::Ts, ContentHash::from_bytes(b"c"), [a]);

        let cycles = graph.find_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0],
            vec![
                PathBuf::from("src/a.ts"),
                PathBuf::from("src/b.ts"),
                PathBuf::from("src/c.ts"),
                PathBuf::from("src/a.ts"),
            ]
        );
    }
}
