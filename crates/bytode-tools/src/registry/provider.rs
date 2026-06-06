#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolProviderId(pub &'static str);

use super::ToolEntry;
use crate::tools::Tool;

pub trait ToolProvider {
    fn provider_id(&self) -> ToolProviderId;
    fn list_tools(&self) -> Vec<&ToolEntry>;
    fn into_tools(self: Box<Self>) -> Vec<ToolEntry>;

    fn id(&self) -> ToolProviderId {
        self.provider_id()
    }

    fn get_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.list_tools()
            .into_iter()
            .find(|entry| entry.descriptor.name == name)
            .map(|entry| &*entry.tool)
    }

    fn get_entry(&self, name: &str) -> Option<&ToolEntry> {
        self.list_tools()
            .into_iter()
            .find(|entry| entry.descriptor.name == name)
    }

    fn tools(self: Box<Self>) -> Vec<ToolEntry> {
        self.into_tools()
    }
}
