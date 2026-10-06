//! Seed definition of the builtin web search worker profile (§6.3).
use crate::worker_agents::contracts::*;

pub(crate) const WEB_SEARCH_CONTEXT: &str =
    include_str!("../../../../.s11tnext/worker-web-search.txt");

const PURPOSE: &str = "Public web research worker: looks up current or recent information on the public internet, \
searches the web, checks news, latest versions, prices, release dates, announcements and facts that may have changed, \
and returns short verified claims with source URLs. Use for questions that need fresh public information. \
公開ウェブの調査: 調べて、検索して、ニュース、最新情報、最近の出来事、今日の天気や相場、リリース日、新製品、\
公式発表、ウェブで確認が必要な事実の確認。出典 URL つきの短い主張を返す。";

/// The profile the host seeds as `revision=1, approved` (see `schema::seed_builtin`).
pub(crate) fn web_search_draft() -> ProfileDraft {
    ProfileDraft {
        profile_id: WEB_SEARCH_PROFILE_ID.to_string(),
        purpose: PURPOSE.to_string(),
        system_context: WEB_SEARCH_CONTEXT.to_string(),
        skill_revision_ids: Vec::new(),
        tools: BUILTIN_TOOL_KEYS
            .iter()
            .map(|key| ToolRef {
                kind: ToolRefKind::Builtin,
                key: (*key).to_string(),
                catalog_revision_id: None,
            })
            .collect(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1, "maxLength": 400},
                "urls": {"type": "array", "maxItems": 3, "items": {"type": "string", "maxLength": 2048}}
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        output_kind: OutputKind::WebClaimsV1,
        output_schema: None,
        completion: CompletionCriteria {
            min_items: 1,
            sources_must_be_host_recorded: true,
        },
        limits: WorkerLimits::default(),
        tier_policy: TierPolicy {
            max_tier: Tier::Local,
            cloud: CloudPolicy::Never,
        },
    }
}
