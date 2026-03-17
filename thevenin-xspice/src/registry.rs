//! Registry for XSPICE code model definitions.

use std::collections::BTreeMap;

use crate::model::CodeModelDef;

/// A registry of XSPICE code model definitions, keyed by uppercase model type name.
#[derive(Default)]
pub struct CodeModelRegistry {
    models: BTreeMap<String, CodeModelDef>,
}

impl CodeModelRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a code model definition. The model's name is uppercased for
    /// case-insensitive lookup.
    pub fn register(&mut self, model: CodeModelDef) {
        self.models.insert(model.name.to_uppercase(), model);
    }

    /// Look up a code model by type name (case-insensitive).
    pub fn get(&self, type_name: &str) -> Option<&CodeModelDef> {
        self.models.get(&type_name.to_uppercase())
    }

    /// Returns true if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Number of registered models.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Iterate over registered model names (uppercased).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.models.keys().map(|s| s.as_str())
    }
}

impl std::fmt::Debug for CodeModelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeModelRegistry")
            .field("models", &self.models.keys().collect::<Vec<_>>())
            .finish()
    }
}
