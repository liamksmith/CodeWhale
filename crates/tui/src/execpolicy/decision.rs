use serde::Deserialize;
use serde::Serialize;

use super::error::Error;
use super::error::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    /// 命令可直接运行，无需进一步审批。
    Allow,
    /// 请求用户明确批准；当使用 `approval_policy="never"` 运行时直接拒绝。
    Prompt,
    /// 命令被阻止，不进行进一步考虑。
    Forbidden,
}

impl Decision {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "allow" => Ok(Self::Allow),
            "prompt" => Ok(Self::Prompt),
            "forbidden" => Ok(Self::Forbidden),
            other => Err(Error::InvalidDecision(other.to_string())),
        }
    }
}
