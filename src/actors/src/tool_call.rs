use clients::tool_defs::{
    CargoCheckInput, CargoTest, CargoTestInput, GatherContext, GatherContextInput, GrepInput,
    GrepTool, InsertAfterLine, InsertAfterLineInput, LenientDeserialize, ReadFile, ReadFileInput,
    StringReplace, StringReplaceInput, Tool, ToolId,
};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: ToolId,
    pub name: String,
    pub json: String,
}
impl ToolCall {
    pub fn get_tool(&self) -> anyhow::Result<Tool> {
        let tool = Tool::from_str(self.name.as_str())?;
        match tool {
            Tool::ReadFile(_) => {
                let input = ReadFileInput::deserialize_lenient(&self.json)?;
                Ok(Tool::ReadFile(ReadFile {
                    id: self.id.id.clone(),
                    input,
                }))
            }
            Tool::InsertAfterLine(_) => {
                let input = InsertAfterLineInput::deserialize_lenient(&self.json)?;
                Ok(Tool::InsertAfterLine(InsertAfterLine {
                    id: self.id.id.clone(),
                    input,
                }))
            }
            Tool::StringReplace(_) => {
                let input = StringReplaceInput::deserialize_lenient(&self.json)?;
                Ok(Tool::StringReplace(StringReplace {
                    id: self.id.id.clone(),
                    input,
                }))
            }
            Tool::CargoCheck(_) => {
                let input = if self.json.is_empty() {
                    CargoCheckInput {
                        include_warnings: None,
                    }
                } else {
                    CargoCheckInput::deserialize_lenient(&self.json)?
                };
                Ok(Tool::CargoCheck(clients::tool_defs::CargoCheck {
                    id: self.id.id.clone(),
                    input,
                }))
            }
            Tool::Grep(_) => {
                let input = GrepInput::deserialize_lenient(&self.json)?;
                Ok(Tool::Grep(GrepTool {
                    id: self.id.id.clone(),
                    input,
                }))
            }
            Tool::CargoTest(_) => {
                let input = if self.json.is_empty() {
                    CargoTestInput {
                        package: None,
                        test_name: None,
                    }
                } else {
                    CargoTestInput::deserialize_lenient(&self.json)?
                };
                Ok(Tool::CargoTest(CargoTest {
                    id: self.id.id.clone(),
                    input,
                }))
            }
            Tool::GatherContext(_) => {
                let input = GatherContextInput::deserialize_lenient(&self.json)?;
                Ok(Tool::GatherContext(GatherContext {
                    input,
                    id: self.id.id.clone(),
                }))
            }
        }
    }
}
