//! Ask a reconnectable GUI for permission and elicitation over ACP-in-Envelope.

use std::time::Duration;

use acp_thread::{AuthorizationKind, PermissionOptions, SelectedPermissionOutcome};
use agent::{DaemonPromptHandler, DaemonPromptWait};
use agent_client_protocol::schema::v1 as acp;
use anyhow::Result;
use futures::{FutureExt as _, select_biased};
use gpui::{App, AsyncApp, Task, WeakEntity};
use rpc::proto;
use serde::Serialize;
use serde::de::DeserializeOwned;
use session_protocol::{AUTHORIZATION_KIND_META, JsonRpcMessage, methods, request};

use crate::SessionHost;
const RETRY_POLL: Duration = Duration::from_secs(1);

pub(crate) struct HostPromptHandler(pub WeakEntity<SessionHost>);

impl DaemonPromptHandler for HostPromptHandler {
    fn request_permission(
        &self,
        session_id: acp::SessionId,
        tool_call: acp::ToolCallUpdate,
        options: PermissionOptions,
        kind: AuthorizationKind,
        cx: &mut App,
    ) -> Task<DaemonPromptWait<SelectedPermissionOutcome>> {
        let host = self.0.clone();
        let request = acp::RequestPermissionRequest::new(
            session_id,
            tool_call,
            acp_permission_options(&options),
        )
        .meta(authorization_kind_meta(kind));
        cx.spawn(async move |cx| {
            match wait_for_gui_rpc::<acp::RequestPermissionResponse>(
                host,
                methods::SESSION_REQUEST_PERMISSION,
                request,
                cx,
            )
            .await
            {
                DaemonPromptWait::Answered(response) => {
                    match selected_permission_outcome(response, &options) {
                        Ok(outcome) => DaemonPromptWait::Answered(outcome),
                        Err(error) => {
                            log::warn!("permission response from GUI: {error:#}");
                            DaemonPromptWait::TimedOut
                        }
                    }
                }
                DaemonPromptWait::TimedOut => DaemonPromptWait::TimedOut,
            }
        })
    }

    fn request_elicitation(
        &self,
        request: acp::CreateElicitationRequest,
        cx: &mut App,
    ) -> Task<DaemonPromptWait<acp::CreateElicitationResponse>> {
        let host = self.0.clone();
        cx.spawn(async move |cx| {
            wait_for_gui_rpc(host, methods::ELICITATION_CREATE, request, cx).await
        })
    }
}

async fn wait_for_gui_rpc<T>(
    host: WeakEntity<SessionHost>,
    method: &'static str,
    params: impl Serialize + Clone + 'static,
    cx: &mut AsyncApp,
) -> DaemonPromptWait<T>
where
    T: DeserializeOwned,
{
    let Ok((mut attached_rx, wait)) = host.read_with(cx, |host, _| {
        (
            host.gui_attached_tx.receiver(),
            host.config.disconnected_prompt_wait,
        )
    }) else {
        return DaemonPromptWait::TimedOut;
    };

    let deadline = wait
        .duration()
        .map(|duration| cx.background_executor().now() + duration);

    loop {
        if deadline.is_some_and(|deadline| cx.background_executor().now() >= deadline) {
            return DaemonPromptWait::TimedOut;
        }

        let prepared = host.update(cx, |host, _cx| {
            host.next_rpc_id += 1;
            let json = request(host.next_rpc_id, method, params.clone())?;
            anyhow::Ok((host.session.clone(), json))
        });
        let Ok(Ok((proto, json))) = prepared else {
            return DaemonPromptWait::TimedOut;
        };

        let send = proto.request(proto::SessionAgentRpc {
            json,
            seq: 0,
            agent_id: String::new(),
        });
        let remaining = remaining_until(deadline, cx);
        let parsed = match remaining {
            Some(remaining) => {
                let timer = cx.background_executor().timer(remaining);
                futures::pin_mut!(send);
                futures::pin_mut!(timer);
                select_biased! {
                    result = send.fuse() => parse_rpc_result::<T>(result),
                    _ = timer.fuse() => return DaemonPromptWait::TimedOut,
                }
            }
            None => parse_rpc_result::<T>(send.await),
        };

        match parsed {
            RpcParse::Answered(value) => return DaemonPromptWait::Answered(value),
            RpcParse::Retry => {
                let remaining = remaining_until(deadline, cx);
                wait_before_retry(&mut attached_rx, remaining, cx).await;
            }
        }
    }
}

enum RpcParse<T> {
    Answered(T),
    Retry,
}

fn parse_rpc_result<T: DeserializeOwned>(
    result: Result<proto::SessionAgentRpc, anyhow::Error>,
) -> RpcParse<T> {
    match result {
        Ok(rpc) => match JsonRpcMessage::parse(&rpc.json).and_then(|parsed| parsed.result_as()) {
            Ok(value) => RpcParse::Answered(value),
            Err(error) => {
                log::debug!("GUI prompt RPC not ready: {error:#}");
                RpcParse::Retry
            }
        },
        Err(error) => {
            log::debug!("GUI prompt RPC skipped (no GUI?): {error:#}");
            RpcParse::Retry
        }
    }
}

fn remaining_until(deadline: Option<std::time::Instant>, cx: &AsyncApp) -> Option<Duration> {
    let deadline = deadline?;
    Some(deadline.saturating_duration_since(cx.background_executor().now()))
}

async fn wait_before_retry(
    attached_rx: &mut watch::Receiver<u64>,
    remaining: Option<Duration>,
    cx: &AsyncApp,
) {
    let poll = remaining.unwrap_or(RETRY_POLL).min(RETRY_POLL);
    if poll.is_zero() {
        return;
    }
    let timer = cx.background_executor().timer(poll);
    let changed = attached_rx.changed();
    futures::pin_mut!(timer);
    futures::pin_mut!(changed);
    select_biased! {
        _ = changed.fuse() => {}
        _ = timer.fuse() => {}
    }
}

fn acp_permission_options(options: &PermissionOptions) -> Vec<acp::PermissionOption> {
    match options {
        PermissionOptions::Flat(options) => options.clone(),
        PermissionOptions::Dropdown(choices) => flatten_choices(choices),
        PermissionOptions::DropdownWithPatterns { choices, .. } => flatten_choices(choices),
    }
}

fn flatten_choices(choices: &[acp_thread::PermissionOptionChoice]) -> Vec<acp::PermissionOption> {
    let mut options = Vec::with_capacity(choices.len().saturating_mul(2));
    for choice in choices {
        options.push(choice.allow.clone());
        options.push(choice.deny.clone());
    }
    options
}

fn authorization_kind_meta(kind: AuthorizationKind) -> acp::Meta {
    let mut meta = acp::Meta::new();
    let value = match kind {
        AuthorizationKind::PermissionGrant => "permission_grant",
        AuthorizationKind::ActionChoice => "action_choice",
    };
    meta.insert(
        AUTHORIZATION_KIND_META.to_string(),
        serde_json::Value::String(value.to_string()),
    );
    meta
}

fn selected_permission_outcome(
    response: acp::RequestPermissionResponse,
    options: &PermissionOptions,
) -> Result<SelectedPermissionOutcome> {
    match response.outcome {
        acp::RequestPermissionOutcome::Selected(selected) => {
            let kind = option_kind_for_id(options, &selected.option_id)
                .unwrap_or(acp::PermissionOptionKind::RejectOnce);
            Ok(SelectedPermissionOutcome::new(selected.option_id, kind))
        }
        acp::RequestPermissionOutcome::Cancelled => {
            anyhow::bail!("permission cancelled by GUI")
        }
        _ => anyhow::bail!("unknown permission outcome"),
    }
}

fn option_kind_for_id(
    options: &PermissionOptions,
    option_id: &acp::PermissionOptionId,
) -> Option<acp::PermissionOptionKind> {
    let matches_id = |option: &acp::PermissionOption| -> bool { &option.option_id == option_id };
    match options {
        PermissionOptions::Flat(options) => options
            .iter()
            .find(|option| matches_id(option))
            .map(|option| option.kind),
        PermissionOptions::Dropdown(choices)
        | PermissionOptions::DropdownWithPatterns { choices, .. } => {
            choices.iter().find_map(|choice| {
                if matches_id(&choice.allow) {
                    Some(choice.allow.kind)
                } else if matches_id(&choice.deny) {
                    Some(choice.deny.kind)
                } else {
                    None
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_dropdown_sends_allow_and_deny() {
        let allow =
            acp::PermissionOption::new("allow-once", "Allow", acp::PermissionOptionKind::AllowOnce);
        let deny = acp::PermissionOption::new(
            "reject-once",
            "Deny",
            acp::PermissionOptionKind::RejectOnce,
        );
        let options = PermissionOptions::Dropdown(vec![acp_thread::PermissionOptionChoice {
            allow: allow.clone(),
            deny: deny.clone(),
            sub_patterns: Vec::new(),
        }]);
        let flat = acp_permission_options(&options);
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].option_id, allow.option_id);
        assert_eq!(flat[1].option_id, deny.option_id);
    }

    #[test]
    fn cancelled_permission_is_not_an_answer() {
        let options = PermissionOptions::Flat(vec![acp::PermissionOption::new(
            "allow-once",
            "Allow",
            acp::PermissionOptionKind::AllowOnce,
        )]);
        let response =
            acp::RequestPermissionResponse::new(acp::RequestPermissionOutcome::Cancelled);
        assert!(selected_permission_outcome(response, &options).is_err());
    }
}
