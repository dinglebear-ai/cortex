//! `cortex reflect` against a Cortex server. The server already holds the
//! transcripts the host agent forwards, so incidents are listed and
//! investigated over its REST API instead of re-indexing locally. The LLM
//! step still runs on this machine (see `CortexService::run_reflect_with`).

use cortex::app::{
    AiHookIncidentRequest, AiHookIncidentResponse, AiHookInvestigateRequest,
    AiHookInvestigateResponse, AiMcpIncidentRequest, AiMcpIncidentResponse,
    AiMcpInvestigateRequest, AiMcpInvestigateResponse, AiSkillIncidentRequest,
    AiSkillIncidentResponse, AiSkillInvestigateRequest, AiSkillInvestigateResponse,
    ReflectIncidentSource, ServiceResult,
};

use super::http_client::HttpClient;

pub(crate) struct HttpReflectSource<'a> {
    client: &'a HttpClient,
}

impl<'a> HttpReflectSource<'a> {
    pub(crate) fn new(client: &'a HttpClient) -> Self {
        Self { client }
    }
}

// HTTP errors become `ServiceError::Internal` through `From<anyhow::Error>`.
// An unknown incident id comes back as an empty evidence list (the server
// runs the same investigate service), which reflect reports as "incident
// changed"; a 404 means the endpoint itself is missing on an older server,
// and its message already says to upgrade.
impl ReflectIncidentSource for HttpReflectSource<'_> {
    fn label(&self) -> String {
        self.client.base_url().to_string()
    }

    async fn list_skill(
        &self,
        req: AiSkillIncidentRequest,
    ) -> ServiceResult<AiSkillIncidentResponse> {
        Ok(self.client.ai_skill_incidents(&req).await?)
    }

    async fn list_mcp(&self, req: AiMcpIncidentRequest) -> ServiceResult<AiMcpIncidentResponse> {
        Ok(self.client.ai_mcp_incidents(&req).await?)
    }

    async fn list_hook(&self, req: AiHookIncidentRequest) -> ServiceResult<AiHookIncidentResponse> {
        Ok(self.client.ai_hook_incidents(&req).await?)
    }

    async fn investigate_skill(
        &self,
        req: AiSkillInvestigateRequest,
    ) -> ServiceResult<AiSkillInvestigateResponse> {
        Ok(self.client.ai_skill_investigate(&req).await?)
    }

    async fn investigate_mcp(
        &self,
        req: AiMcpInvestigateRequest,
    ) -> ServiceResult<AiMcpInvestigateResponse> {
        Ok(self.client.ai_mcp_investigate(&req).await?)
    }

    async fn investigate_hook(
        &self,
        req: AiHookInvestigateRequest,
    ) -> ServiceResult<AiHookInvestigateResponse> {
        Ok(self.client.ai_hook_investigate(&req).await?)
    }
}
