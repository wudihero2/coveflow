#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Run {
    pub id: uuid::Uuid,
    pub workspace_id: String,
    pub kind: RunKind,
    pub script_hash: Option<String>,
    pub script_path: Option<String>,
    pub raw_code: Option<String>,
    pub language: Option<crate::ScriptLang>,
    pub args: Option<serde_json::Value>,
    pub tag: String,
    pub parent_run: Option<uuid::Uuid>,
    pub root_run: Option<uuid::Uuid>,
    pub requirements: Vec<String>,
    pub timeout: Option<i32>,
    pub custom_image: Option<String>,
    pub created_by: String,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Script,
    Flow,
    Preview,
    FlowPreview,
    Maintenance,
}

impl RunKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Flow => "flow",
            Self::Preview => "preview",
            Self::FlowPreview => "flow_preview",
            Self::Maintenance => "maintenance",
        }
    }
}

impl std::fmt::Display for RunKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for RunKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "script" => Ok(Self::Script),
            "flow" => Ok(Self::Flow),
            "preview" => Ok(Self::Preview),
            "flow_preview" => Ok(Self::FlowPreview),
            "maintenance" => Ok(Self::Maintenance),
            other => Err(format!("invalid run kind: {other}")),
        }
    }
}
