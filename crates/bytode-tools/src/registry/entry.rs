use crate::descriptor::ToolDescriptor;
use crate::tools::{Tool, ToolAvailability};

pub struct ToolEntry {
    pub descriptor: ToolDescriptor,
    pub tool: Box<dyn Tool>,
    pub enabled: bool,
    pub availability: ToolAvailability,
}

impl ToolEntry {
    pub fn new(tool: Box<dyn Tool>, availability: ToolAvailability) -> Self {
        let descriptor = tool.descriptor();
        Self {
            descriptor,
            tool,
            enabled: false,
            availability,
        }
    }
}
