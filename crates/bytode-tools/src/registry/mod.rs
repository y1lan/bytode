mod entry;
mod provider;

pub use entry::ToolEntry;
pub use provider::ToolProviderId;

use crate::descriptor::{ToolCapability, ToolDescriptor};
use crate::tools::ToolAvailability;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub struct ToolRegistry {
    all: Vec<ToolEntry>,
    name_to_index: HashMap<String, usize>,
}

impl ToolRegistry {
    pub fn new(tools: Vec<ToolEntry>) -> Self {
        let mut name_to_index = HashMap::new();
        for (index, entry) in tools.iter().enumerate() {
            name_to_index.insert(entry.descriptor.name.to_string(), index);
        }

        Self {
            all: tools,
            name_to_index,
        }
    }

    pub fn activate_for(
        &mut self,
        primary_language: &str,
        detected_languages: &HashSet<String>,
        enabled: &HashSet<String>,
        disabled: &HashSet<String>,
        is_exact: bool,
    ) {
        for entry in &mut self.all {
            let name = entry.descriptor.name;

            if disabled.contains(name) {
                entry.enabled = false;
                continue;
            }

            if is_exact {
                entry.enabled = enabled.contains(name);
                continue;
            }

            let allowed = match &entry.availability {
                ToolAvailability::Always => true,
                ToolAvailability::PrimaryLanguage { requires } => {
                    requires.contains(&primary_language)
                }
                ToolAvailability::DetectedLanguage { languages } => languages
                    .iter()
                    .any(|language| detected_languages.contains(*language)),
            };

            entry.enabled = allowed || enabled.contains(name);
        }
    }

    pub fn get(&self, name: &str) -> Option<&ToolEntry> {
        self.name_to_index.get(name).map(|index| &self.all[*index])
    }

    pub fn find(&self, name: &str) -> Option<&dyn crate::tools::Tool> {
        let entry = self.get(name)?;
        if entry.enabled {
            Some(&*entry.tool)
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut ToolEntry> {
        let index = *self.name_to_index.get(name)?;
        self.all.get_mut(index)
    }

    pub fn descriptors(&self) -> Vec<&ToolDescriptor> {
        self.all.iter().map(|entry| &entry.descriptor).collect()
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        match self.get_mut(name) {
            Some(entry) => {
                entry.enabled = enabled;
                true
            }
            None => false,
        }
    }

    pub fn by_capability(&self, capability: ToolCapability) -> Vec<&ToolEntry> {
        self.all
            .iter()
            .filter(|entry| entry.descriptor.capabilities.contains(&capability))
            .collect()
    }

    pub fn active_names(&self) -> Vec<String> {
        self.all
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.descriptor.name.to_string())
            .collect()
    }

    pub fn to_openai_format(&self) -> Vec<Value> {
        self.all
            .iter()
            .filter(|entry| entry.enabled)
            .map(|entry| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": entry.descriptor.name,
                        "description": entry.descriptor.description,
                        "parameters": entry.tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }
}
