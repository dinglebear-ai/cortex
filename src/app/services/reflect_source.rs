//! Where `cortex reflect` gets incidents from. The local database
//! (`CortexService`) is one source; the CLI implements another over a
//! Cortex server's HTTP API, which already holds forwarded transcripts.
//! Listing and investigation happen at the source; the LLM step always
//! runs locally.

use std::future::Future;

use super::*;
use crate::app::models::{
    AiHookIncidentRequest, AiHookIncidentResponse, AiHookInvestigateRequest,
    AiHookInvestigateResponse, AiMcpIncidentRequest, AiMcpIncidentResponse,
    AiMcpInvestigateRequest, AiMcpInvestigateResponse, AiSkillIncidentRequest,
    AiSkillIncidentResponse, AiSkillInvestigateRequest, AiSkillInvestigateResponse,
};

pub trait ReflectIncidentSource: Sync {
    /// Shown in the report: `local`, or the server URL.
    fn label(&self) -> String;

    fn list_skill(
        &self,
        req: AiSkillIncidentRequest,
    ) -> impl Future<Output = ServiceResult<AiSkillIncidentResponse>> + Send;

    fn list_mcp(
        &self,
        req: AiMcpIncidentRequest,
    ) -> impl Future<Output = ServiceResult<AiMcpIncidentResponse>> + Send;

    fn list_hook(
        &self,
        req: AiHookIncidentRequest,
    ) -> impl Future<Output = ServiceResult<AiHookIncidentResponse>> + Send;

    fn investigate_skill(
        &self,
        req: AiSkillInvestigateRequest,
    ) -> impl Future<Output = ServiceResult<AiSkillInvestigateResponse>> + Send;

    fn investigate_mcp(
        &self,
        req: AiMcpInvestigateRequest,
    ) -> impl Future<Output = ServiceResult<AiMcpInvestigateResponse>> + Send;

    fn investigate_hook(
        &self,
        req: AiHookInvestigateRequest,
    ) -> impl Future<Output = ServiceResult<AiHookInvestigateResponse>> + Send;
}

impl ReflectIncidentSource for CortexService {
    fn label(&self) -> String {
        "local".to_string()
    }

    async fn list_skill(
        &self,
        req: AiSkillIncidentRequest,
    ) -> ServiceResult<AiSkillIncidentResponse> {
        self.list_ai_skill_incidents(req).await
    }

    async fn list_mcp(&self, req: AiMcpIncidentRequest) -> ServiceResult<AiMcpIncidentResponse> {
        self.list_ai_mcp_incidents(req).await
    }

    async fn list_hook(&self, req: AiHookIncidentRequest) -> ServiceResult<AiHookIncidentResponse> {
        self.list_ai_hook_incidents(req).await
    }

    async fn investigate_skill(
        &self,
        req: AiSkillInvestigateRequest,
    ) -> ServiceResult<AiSkillInvestigateResponse> {
        self.investigate_ai_skill_incidents(req).await
    }

    async fn investigate_mcp(
        &self,
        req: AiMcpInvestigateRequest,
    ) -> ServiceResult<AiMcpInvestigateResponse> {
        self.investigate_ai_mcp_incidents(req).await
    }

    async fn investigate_hook(
        &self,
        req: AiHookInvestigateRequest,
    ) -> ServiceResult<AiHookInvestigateResponse> {
        self.investigate_ai_hook_incidents(req).await
    }
}
