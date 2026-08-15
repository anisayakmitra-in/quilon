//! The lexical environment: a stack of scopes mapping a name to what it was bound to.

use super::*;

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            scopes: vec![HashMap::new()],
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    pub fn define(
        &mut self,
        name: String,
        type_: Type,
        mutable: bool,
        span: Span,
    ) -> Result<(), TypeError> {
        let current_scope = self.scopes.last_mut().unwrap();

        if current_scope.contains_key(&name) {
            return Err(TypeError::DuplicateDefinition { name, span });
        }

        current_scope.insert(
            name,
            Symbol {
                type_,
                mutable,
                span,
            },
        );
        Ok(())
    }

    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for scope in self.scopes.iter().rev() {
            if let Some(symbol) = scope.get(name) {
                return Some(symbol);
            }
        }
        None
    }

    pub fn get_type(&self, name: &str) -> Option<Type> {
        self.lookup(name).map(|s| s.type_.clone())
    }

    pub fn is_mutable(&self, name: &str) -> bool {
        self.lookup(name).map(|s| s.mutable).unwrap_or(false)
    }

    /// Update a binding's type (used for function type inference).
    /// Returns `true` if a binding was found and updated.
    pub fn update_type(&mut self, name: &str, new_type: Type) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(symbol) = scope.get_mut(name) {
                symbol.type_ = new_type;
                return true;
            }
        }
        false
    }
}
