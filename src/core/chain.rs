//! Recursive orchestration of saved-request chaining.
//!
//! `api.requests.execute(ChatAdmin.Login)` records a saved-request reference in
//! the script output (see `script.rs`). This module runs those references
//! through the *same* request lifecycle the app would use when sending that
//! saved request normally: its saved template, its own pre/post-response
//! scripts, variable resolution, normal HTTP execution (locally or via the
//! configured local/server execution policy), environment mutations, normal
//! history and sanitized shared history.
//!
//! The runner is deliberately UI-free so it can be unit-tested end to end with
//! a loopback listener. The application layer supplies the actual HTTP sender
//! (constructed for the active local or upstream workspace) and consumes the
//! returned history entries, shared-history payloads and environment
//! mutations.
//!
//! Chaining is bounded: cycles are detected by stable saved-request identity
//! and both nesting depth and the total number of chained executions are
//! capped ([`crate::core::request_namespace::CHAIN_MAX_DEPTH`] and
//! [`CHAIN_MAX_TOTAL`]). The whole chain shares one cancellation token.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{
    history::HistoryEntry,
    request::{RequestDraft, RequestError, ResponseData},
    request_namespace::{CHAIN_MAX_DEPTH, CHAIN_MAX_TOTAL, RequestNamespaceCatalog},
    script::{
        ChainedRequest, EnvironmentMutation, InlineChainer, SCRIPT_TIMEOUT, ScriptCancellation,
        ScriptScope, execute_post_response_with_chain, execute_pre_request_with_chain,
    },
    template::resolve_request,
    upstream_management::SharedHistoryUpload,
    workspace::Workspace,
};

pub trait ChainRequestExecution: Send + Sync {
    fn execution_limits(&self) -> Option<&super::execution_limits::ExecutionLimits>;
    fn send(
        &self,
        request: RequestDraft,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<ResponseData, RequestError>> + Send + '_>>;
}

/// Preparation happens for the actual target ID before its pre-script.
pub trait ChainExecutor: Send + Sync {
    fn prepare(
        &self,
        request_id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Result<Box<dyn ChainRequestExecution + '_>, RequestError>>
                + Send
                + '_,
        >,
    >;
}

struct BoundInlineChainer<'a> {
    inner: &'a dyn InlineChainer,
    depth: usize,
}

impl InlineChainer for BoundInlineChainer<'_> {
    fn run(&self, requested: &[ChainedRequest]) -> Vec<Result<ChainRun, String>> {
        self.run_with_budget(requested, 0, None)
    }

    fn run_at_depth(
        &self,
        requested: &[ChainedRequest],
        depth: usize,
    ) -> Vec<Result<ChainRun, String>> {
        self.run_with_budget(requested, depth, None)
    }

    fn run_with_budget(
        &self,
        requested: &[ChainedRequest],
        depth: usize,
        deadline: Option<std::time::Instant>,
    ) -> Vec<Result<ChainRun, String>> {
        self.inner
            .run_with_budget(requested, self.depth.saturating_add(depth), deadline)
    }
}

/// Drive borrowed pipelines off the caller's Tokio context. The independent
/// watchdog is essential: a synchronous QuickJS child can monopolize a future
/// poll, preventing timeout_at itself from being polled. Cancel only this batch's
/// linked token, then join the worker so no child user code survives the call.
pub(crate) fn drive_inline_with_budget<T: Send>(
    runtime: &tokio::runtime::Runtime,
    cancellation: &ScriptCancellation,
    deadline: Option<std::time::Instant>,
    future: impl Future<Output = T> + Send,
) -> std::thread::Result<Option<T>> {
    std::thread::scope(|scope| {
        let (finished, completion) = std::sync::mpsc::channel::<()>();
        if let Some(deadline) = deadline {
            scope.spawn(move || {
                if matches!(
                    completion.recv_timeout(
                        deadline.saturating_duration_since(std::time::Instant::now())
                    ),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                ) {
                    cancellation.cancel();
                }
            });
        }
        let worker = scope.spawn(move || {
            let _finished = finished;
            runtime.block_on(async {
                match deadline {
                    Some(deadline) => {
                        match tokio::time::timeout_at(deadline.into(), future).await {
                            Ok(result) => Some(result),
                            Err(_) => {
                                cancellation.cancel();
                                None
                            }
                        }
                    }
                    None => Some(future.await),
                }
            })
        });
        worker.join()
    })
}

struct LegacyExecutor<S>(S);
struct LegacyExecution<'a, S>(&'a S);

impl<S, Fut> ChainExecutor for LegacyExecutor<S>
where
    S: Fn(RequestDraft) -> Fut + Send + Sync,
    Fut: Future<Output = Result<ResponseData, RequestError>> + Send + 'static,
{
    fn prepare(
        &self,
        _: &str,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Result<Box<dyn ChainRequestExecution + '_>, RequestError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(
            async move { Ok(Box::new(LegacyExecution(&self.0)) as Box<dyn ChainRequestExecution>) },
        )
    }
}

impl<S, Fut> ChainRequestExecution for LegacyExecution<'_, S>
where
    S: Fn(RequestDraft) -> Fut + Send + Sync,
    Fut: Future<Output = Result<ResponseData, RequestError>> + Send + 'static,
{
    fn execution_limits(&self) -> Option<&super::execution_limits::ExecutionLimits> {
        None
    }
    fn send(
        &self,
        request: RequestDraft,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<ResponseData, RequestError>> + Send + '_>>
    {
        Box::pin((self.0)(request))
    }
}

/// Result of running a chain (or the chain prefix that ran before a failure).
#[derive(Clone, Debug, Default)]
pub struct ChainRun {
    pub history: Vec<HistoryEntry>,
    pub shared_history: Vec<SharedHistoryUpload>,
    /// Mutations produced by the chained requests that successfully applied
    /// them, in execution order.
    pub environment_mutations: Vec<EnvironmentMutation>,
    /// The failure that stopped the chain, if any.
    pub error: Option<ChainFailure>,
}

#[derive(Clone, Debug)]
pub struct ChainFailure {
    /// Dotted access path of the chained request that failed, e.g.
    /// `ChatAdmin.Refresh`. `None` when the failure is not tied to one
    /// chained request.
    pub path: Option<String>,
    /// Human-facing message. Callers redact secrets before surfacing it.
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainLimits {
    pub max_depth: usize,
    pub max_total: usize,
    /// Execution budget for each chained request's own pre/post-response
    /// scripts, inherited from the caller's configured script timeout.
    pub script_timeout: std::time::Duration,
}

impl Default for ChainLimits {
    fn default() -> Self {
        Self {
            max_depth: CHAIN_MAX_DEPTH,
            max_total: CHAIN_MAX_TOTAL,
            script_timeout: SCRIPT_TIMEOUT,
        }
    }
}

/// Run a sequence of scheduled chained requests in order, recursively.
///
/// `scheduled` are the references recorded by the current script phase.
/// `budget` counts the total number of chained executions across the whole
/// top-level Send (shared between the pre- and post-response chains). It must
/// be zeroed at the start of each Send. `sender` performs the actual HTTP
/// exchange for one resolved request.
#[allow(clippy::too_many_arguments)]
pub async fn run_chain<S, Fut>(
    workspace: &Workspace,
    environment_id: Option<&str>,
    scheduled: &[ChainedRequest],
    namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
    chain_inline: Option<&dyn InlineChainer>,
    sender: S,
    limits: ChainLimits,
    budget: &AtomicUsize,
) -> ChainRun
where
    S: Fn(RequestDraft) -> Fut + Send + Sync,
    Fut: Future<Output = Result<ResponseData, RequestError>> + Send + 'static,
{
    run_chain_with_executor(
        workspace,
        environment_id,
        scheduled,
        namespace,
        cancellation,
        chain_inline,
        &LegacyExecutor(sender),
        limits,
        budget,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_chain_with_executor(
    workspace: &Workspace,
    environment_id: Option<&str>,
    scheduled: &[ChainedRequest],
    namespace: &RequestNamespaceCatalog,
    cancellation: &ScriptCancellation,
    chain_inline: Option<&dyn InlineChainer>,
    sender: &dyn ChainExecutor,
    limits: ChainLimits,
    budget: &AtomicUsize,
) -> ChainRun {
    let mut run = ChainRun::default();
    let mut working = workspace.clone();
    let mut stack: Vec<String> = Vec::new();
    for next in scheduled {
        execute_chained(
            &mut run,
            &mut working,
            environment_id,
            next,
            namespace,
            cancellation,
            chain_inline,
            sender,
            limits,
            &mut stack,
            1,
            budget,
        )
        .await;
        if run.error.is_some() || cancellation.is_cancelled() {
            break;
        }
    }
    run
}

#[allow(clippy::too_many_arguments)]
fn execute_chained<'a>(
    run: &'a mut ChainRun,
    workspace: &'a mut Workspace,
    environment_id: Option<&'a str>,
    scheduled: &'a ChainedRequest,
    namespace: &'a RequestNamespaceCatalog,
    cancellation: &'a ScriptCancellation,
    chain_inline: Option<&'a dyn InlineChainer>,
    sender: &'a dyn ChainExecutor,
    limits: ChainLimits,
    stack: &'a mut Vec<String>,
    depth: usize,
    budget: &'a AtomicUsize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if run.error.is_some() {
            return;
        }
        if cancellation.is_cancelled() {
            run.error = Some(ChainFailure {
                path: Some(scheduled.path.clone()),
                message: "Request cancelled".to_owned(),
            });
            return;
        }
        if depth > limits.max_depth {
            run.error = Some(ChainFailure {
                path: Some(scheduled.path.clone()),
                message: format!(
                    "Chained execution exceeded the maximum nesting depth of {} at '{}'.",
                    limits.max_depth, scheduled.path
                ),
            });
            return;
        }
        if budget.load(Ordering::Relaxed) >= limits.max_total {
            run.error = Some(ChainFailure {
                path: Some(scheduled.path.clone()),
                message: format!(
                    "Chained execution exceeded the maximum of {} total chained requests (at '{}').",
                    limits.max_total, scheduled.path
                ),
            });
            return;
        }
        if stack.iter().any(|id| id == &scheduled.id) {
            let mut cycle_paths = stack.clone();
            cycle_paths.push(scheduled.id.clone());
            run.error = Some(ChainFailure {
                path: Some(scheduled.path.clone()),
                message: format!(
                    "Request chaining cycle detected: {}",
                    cycle_display(&cycle_paths)
                ),
            });
            return;
        }

        let Some((_collection, saved)) = workspace.saved_request(&scheduled.id) else {
            run.error = Some(ChainFailure {
                path: Some(scheduled.path.clone()),
                message: format!(
                    "Saved request reference '{}' is no longer present in the active workspace.",
                    scheduled.path
                ),
            });
            return;
        };
        let saved_id = saved.id.clone();
        let template = saved.definition.clone();
        // Awaited batches can enter concurrently; admission and increment must
        // be one operation, not a check followed by fetch_add.
        if budget
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < limits.max_total).then(|| used.saturating_add(1))
            })
            .is_err()
        {
            run.error = Some(ChainFailure {
                path: Some(scheduled.path.clone()),
                message: format!(
                    "Chained execution exceeded the maximum of {} total chained requests.",
                    limits.max_total
                ),
            });
            return;
        }

        let prepared = match cancellation_drive(sender.prepare(&saved_id), cancellation).await {
            Ok(prepared) => prepared,
            Err(error) => {
                run.error = Some(ChainFailure {
                    path: Some(scheduled.path.clone()),
                    message: format!("Could not prepare chained execution: {error}"),
                });
                return;
            }
        };
        // Pre-request script (may itself schedule more requests).
        let mut scope = scope_from_workspace(workspace, environment_id, limits.script_timeout);
        scope.execution_limits = prepared.execution_limits().cloned();
        let scoped_inline = chain_inline.map(|chainer| BoundInlineChainer {
            inner: chainer,
            depth,
        });
        let script_inline = scoped_inline
            .as_ref()
            .map(|chainer| chainer as &dyn InlineChainer);
        let pre = match execute_pre_request_with_chain(
            &template.scripts.pre_request,
            &template.request,
            &scope,
            namespace,
            cancellation,
            script_inline,
        ) {
            Ok(result) => result,
            Err(error) => {
                let message = format!(
                    "Pre-request script for '{}' failed: {error}",
                    scheduled.path
                );
                // The script engine already redacted `error`; no extra sensitive
                // values are known at this stage.
                record_failed(run, &template.request, &message, &[]);
                run.error = Some(ChainFailure {
                    path: Some(scheduled.path.clone()),
                    message,
                });
                return;
            }
        };
        if apply_and_collect_mutations(run, workspace, environment_id, &pre.environment_mutations)
            .is_err()
        {
            return;
        }

        stack.push(saved_id.clone());
        if run.error.is_none() {
            for child in &pre.chained_requests {
                execute_chained(
                    &mut *run,
                    &mut *workspace,
                    environment_id,
                    child,
                    namespace,
                    cancellation,
                    chain_inline,
                    sender,
                    limits,
                    &mut *stack,
                    depth + 1,
                    budget,
                )
                .await;
                if run.error.is_some() || cancellation.is_cancelled() {
                    break;
                }
            }
        }
        if run.error.is_some() || cancellation.is_cancelled() {
            stack.pop();
            return;
        }

        // Resolve against the now-current environment and send through the
        // normal execution policy (local or server) provided by the caller.
        let resolved = match resolve_request(
            &pre.request,
            environment_id.and_then(|id| workspace.environment(id)),
        ) {
            Ok(resolved) => resolved,
            Err(error) => {
                stack.pop();
                run.error = Some(ChainFailure {
                    path: Some(scheduled.path.clone()),
                    message: format!("Failed to resolve request '{}': {error}", scheduled.path),
                });
                return;
            }
        };
        let request = resolved.request.clone();
        let sensitive_values = resolved.sensitive_values;
        let network = cancellation_drive(prepared.send(request.clone()), cancellation).await;

        let response = match network {
            Ok(response) => response,
            Err(RequestError::Cancelled) => {
                stack.pop();
                run.error = Some(ChainFailure {
                    path: Some(scheduled.path.clone()),
                    message: "Request cancelled".to_owned(),
                });
                return;
            }
            Err(error) => {
                stack.pop();
                let message = format!(
                    "Chained request '{}' failed to send: {error}",
                    scheduled.path
                );
                record_failed(run, &request, &message, &sensitive_values);
                run.error = Some(ChainFailure {
                    path: Some(scheduled.path.clone()),
                    message,
                });
                return;
            }
        };

        // A non-2xx HTTP response is a normal response, not a transport failure.
        push_history(run, &request, &response, &sensitive_values);

        // Post-response script (may itself schedule more requests).
        let mut post_scope = scope_from_workspace(workspace, environment_id, limits.script_timeout);
        post_scope.execution_limits = prepared.execution_limits().cloned();
        let post = match execute_post_response_with_chain(
            &template.scripts.post_response,
            &request,
            &response,
            &post_scope,
            namespace,
            cancellation,
            script_inline,
        ) {
            Ok(result) => result,
            Err(error) => {
                let message = format!(
                    "Post-response script for '{}' failed: {error}",
                    scheduled.path
                );
                run.error = Some(ChainFailure {
                    path: Some(scheduled.path.clone()),
                    message,
                });
                stack.pop();
                return;
            }
        };
        if apply_and_collect_mutations(run, workspace, environment_id, &post.environment_mutations)
            .is_err()
        {
            stack.pop();
            return;
        }

        if run.error.is_none() {
            for child in &post.chained_requests {
                execute_chained(
                    &mut *run,
                    &mut *workspace,
                    environment_id,
                    child,
                    namespace,
                    cancellation,
                    chain_inline,
                    sender,
                    limits,
                    &mut *stack,
                    depth + 1,
                    budget,
                )
                .await;
                if run.error.is_some() || cancellation.is_cancelled() {
                    break;
                }
            }
        }
        stack.pop();
    })
}

fn cycle_display(paths: &[String]) -> String {
    // heuristic display: we only track request ids on the stack, so present
    // the repeated tail using the reference path that triggered it.
    if paths.len() >= 2 {
        // The caller appends the repeated id; we render it as "... -> <id>".
        format!(
            "{} -> {}",
            paths[..paths.len() - 1].join(" -> "),
            paths[paths.len() - 1]
        )
    } else {
        paths.join(" -> ")
    }
}

/// Build a `ScriptScope` from the active environment of a workspace snapshot.
fn scope_from_workspace(
    workspace: &Workspace,
    environment_id: Option<&str>,
    script_timeout: std::time::Duration,
) -> ScriptScope {
    let mut scope = ScriptScope::default();
    scope.script_timeout = script_timeout;
    if let Some(environment) = environment_id.and_then(|id| workspace.environment(id)) {
        for variable in environment
            .variables
            .iter()
            .filter(|variable| variable.enabled)
        {
            if variable.secret {
                scope
                    .environment
                    .insert_secret(variable.key.clone(), variable.value.clone());
            } else {
                scope
                    .environment
                    .insert(variable.key.clone(), variable.value.clone());
            }
        }
    }
    scope
}

/// Apply script mutations to the working workspace snapshot (so later chained
/// requests and the parent resolve with them) and accumulate them for the
/// caller to persist to the real workspace.
fn apply_and_collect_mutations(
    run: &mut ChainRun,
    workspace: &mut Workspace,
    environment_id: Option<&str>,
    mutations: &[EnvironmentMutation],
) -> Result<(), ()> {
    if mutations.is_empty() {
        return Ok(());
    }
    let Some(environment_id) = environment_id else {
        // No active environment: mutations are transient, matching normal flow.
        return Ok(());
    };
    if let Err(message) = super::workspace::apply_environment_mutations_to_workspace(
        workspace,
        environment_id,
        mutations,
    ) {
        run.error = Some(ChainFailure {
            path: None,
            message,
        });
        return Err(());
    }
    run.environment_mutations.extend(mutations.iter().cloned());
    Ok(())
}

fn push_history(
    run: &mut ChainRun,
    request: &RequestDraft,
    response: &ResponseData,
    sensitive_values: &[String],
) {
    let entry = HistoryEntry::completed_with_secrets(request, response, sensitive_values);
    let shared = SharedHistoryUpload::completed(
        entry.id.clone(),
        entry.created_at,
        request,
        response,
        sensitive_values,
    );
    run.history.push(entry);
    run.shared_history.push(shared);
}

fn record_failed(
    run: &mut ChainRun,
    request: &RequestDraft,
    message: &str,
    sensitive_values: &[String],
) {
    let entry = HistoryEntry::failed_with_secrets(request, message, sensitive_values);
    let shared = SharedHistoryUpload::failed(
        entry.id.clone(),
        entry.created_at,
        request,
        message,
        sensitive_values,
    );
    run.history.push(entry);
    run.shared_history.push(shared);
}

/// Await a chained send, polling the shared cancellation token so cancelling
/// the top-level Send aborts an in-flight chained HTTP request and prevents
/// the rest of the chain from starting.
async fn cancellation_drive<F, T>(
    future: F,
    cancellation: &ScriptCancellation,
) -> Result<T, RequestError>
where
    F: Future<Output = Result<T, RequestError>>,
{
    let mut future = Box::pin(future);
    loop {
        tokio::select! {
            biased;
            result = &mut future => return result,
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                if cancellation.is_cancelled() {
                    return Err(RequestError::Cancelled);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        HeaderEntry,
        request::RequestDraft,
        template::RequestTemplate,
        workspace::{RequestScripts, Workspace},
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn template(url: &str, method: &str, pre: &str, post: &str) -> RequestTemplate {
        RequestTemplate {
            request: RequestDraft::new(method, url),
            scripts: RequestScripts {
                pre_request: pre.to_owned(),
                post_response: post.to_owned(),
            },
            documentation: String::new(),
            websocket: None,
        }
    }

    #[test]
    fn inline_binding_preserves_depth_and_absolute_deadline() {
        struct Probe(Mutex<Option<(usize, Option<std::time::Instant>)>>);
        impl InlineChainer for Probe {
            fn run(&self, _: &[ChainedRequest]) -> Vec<Result<ChainRun, String>> {
                panic!("budget must not be erased")
            }
            fn run_with_budget(
                &self,
                _: &[ChainedRequest],
                depth: usize,
                deadline: Option<std::time::Instant>,
            ) -> Vec<Result<ChainRun, String>> {
                *self.0.lock().unwrap() = Some((depth, deadline));
                Vec::new()
            }
        }
        let probe = Probe(Mutex::new(None));
        let outer = BoundInlineChainer {
            inner: &probe,
            depth: 2,
        };
        let inner = BoundInlineChainer {
            inner: &outer,
            depth: 3,
        };
        let deadline = Some(std::time::Instant::now());
        inner.run_with_budget(&[], 4, deadline);
        assert_eq!(*probe.0.lock().unwrap(), Some((9, deadline)));
    }

    #[test]
    fn inline_parent_budget_bounds_unlimited_children() {
        use crate::core::execution_limits::{Bound, ExecutionLimits};
        use crate::core::script::ScriptErrorKind;
        use std::time::{Duration, Instant};

        struct Stalled {
            limits: ExecutionLimits,
            dropped: Arc<std::sync::atomic::AtomicBool>,
        }
        impl ChainRequestExecution for Stalled {
            fn execution_limits(&self) -> Option<&ExecutionLimits> {
                Some(&self.limits)
            }
            fn send(
                &self,
                _: RequestDraft,
            ) -> std::pin::Pin<
                Box<dyn Future<Output = Result<ResponseData, RequestError>> + Send + '_>,
            > {
                struct MarkDrop(Arc<std::sync::atomic::AtomicBool>);
                impl Drop for MarkDrop {
                    fn drop(&mut self) {
                        self.0.store(true, Ordering::Release);
                    }
                }
                Box::pin(async {
                    let _drop = MarkDrop(self.dropped.clone());
                    // Finite safety net: a regression fails instead of hanging.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Err(RequestError::Cancelled)
                })
            }
        }
        impl ChainExecutor for Stalled {
            fn prepare(
                &self,
                _: &str,
            ) -> std::pin::Pin<
                Box<
                    dyn Future<Output = Result<Box<dyn ChainRequestExecution + '_>, RequestError>>
                        + Send
                        + '_,
                >,
            > {
                Box::pin(async {
                    Ok(Box::new(Stalled {
                        limits: self.limits.clone(),
                        dropped: self.dropped.clone(),
                    }) as Box<dyn ChainRequestExecution>)
                })
            }
        }
        struct Runner {
            runtime: tokio::runtime::Runtime,
            workspace: Workspace,
            catalog: RequestNamespaceCatalog,
            cancellation: ScriptCancellation,
            sender: Stalled,
        }
        impl InlineChainer for Runner {
            fn run(&self, requested: &[ChainedRequest]) -> Vec<Result<ChainRun, String>> {
                self.run_with_budget(requested, 0, None)
            }
            fn run_with_budget(
                &self,
                requested: &[ChainedRequest],
                _: usize,
                deadline: Option<Instant>,
            ) -> Vec<Result<ChainRun, String>> {
                let child = self.cancellation.child();
                let budget = AtomicUsize::new(0);
                let run = drive_inline_with_budget(
                    &self.runtime,
                    &child,
                    deadline,
                    run_chain_with_executor(
                        &self.workspace,
                        None,
                        requested,
                        &self.catalog,
                        &child,
                        None,
                        &self.sender,
                        ChainLimits::default(),
                        &budget,
                    ),
                )
                .expect("inline worker should not panic");
                vec![run.ok_or_else(|| "timed out".to_owned())]
            }
        }

        // CPU children run synchronously inside future.poll: the independent
        // watchdog must interrupt them even when Tokio cannot poll its timer.
        for cpu in [false, true] {
            for user_cancel in [false, true] {
                let mut workspace = Workspace::default();
                let col = workspace.create_collection("Root").unwrap();
                workspace
                    .create_saved_request(
                        &col,
                        "Child",
                        template(
                            "https://unused.invalid",
                            "GET",
                            if cpu { "while (true) {}" } else { "" },
                            "",
                        ),
                    )
                    .unwrap();
                let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
                let cancellation = ScriptCancellation::new();
                let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let runner = Runner {
                    runtime: tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap(),
                    workspace,
                    catalog,
                    cancellation: cancellation.clone(),
                    sender: Stalled {
                        limits: ExecutionLimits(
                            [
                                ("http.timeout_ms".to_owned(), Bound::unlimited()),
                                ("script.timeout_ms".to_owned(), Bound::unlimited()),
                            ]
                            .into_iter()
                            .collect(),
                        ),
                        dropped: dropped.clone(),
                    },
                };
                let scope = ScriptScope {
                    execution_limits: Some(ExecutionLimits(
                        [(
                            "script.timeout_ms".to_owned(),
                            if user_cancel {
                                Bound::unlimited()
                            } else {
                                Bound::limited(50)
                            },
                        )]
                        .into_iter()
                        .collect(),
                    )),
                    ..ScriptScope::default()
                };
                // Also bounds the CPU regression if the watchdog breaks.
                let (done, completion) = std::sync::mpsc::channel::<()>();
                let fallback = cancellation.clone();
                let canceller = std::thread::spawn(move || {
                    if completion
                        .recv_timeout(if user_cancel {
                            Duration::from_millis(50)
                        } else {
                            Duration::from_secs(2)
                        })
                        .is_err()
                    {
                        fallback.cancel();
                    }
                });
                let started = Instant::now();
                let error = execute_pre_request_with_chain(
                    "await api.requests.execute(Root.Child);",
                    &RequestDraft::new("GET", "https://unused.invalid"),
                    &scope,
                    &runner.catalog,
                    &cancellation,
                    Some(&runner),
                )
                .expect_err("await must be interrupted");
                done.send(()).ok();
                canceller.join().unwrap();
                assert!(
                    started.elapsed() < Duration::from_secs(1),
                    "cpu={cpu}, cancel={user_cancel}"
                );
                assert_eq!(
                    error.diagnostic.kind,
                    if user_cancel {
                        ScriptErrorKind::Cancelled
                    } else {
                        ScriptErrorKind::TimedOut
                    }
                );
                assert_eq!(
                    cancellation.is_cancelled(),
                    user_cancel,
                    "deadline must not cancel root"
                );
                if !cpu {
                    assert!(
                        dropped.load(Ordering::Acquire),
                        "network future must be dropped"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn scoped_preparation_uses_target_identity_and_fails_before_script() {
        struct Denied(Mutex<Vec<String>>);
        impl ChainExecutor for Denied {
            fn prepare(
                &self,
                id: &str,
            ) -> std::pin::Pin<
                Box<
                    dyn Future<Output = Result<Box<dyn ChainRequestExecution + '_>, RequestError>>
                        + Send
                        + '_,
                >,
            > {
                self.0.lock().unwrap().push(id.to_owned());
                Box::pin(async { Err(RequestError::Upstream("policy denied".to_owned())) })
            }
        }
        let mut workspace = Workspace::default();
        let root = workspace.create_collection("Root").unwrap();
        let child = workspace
            .create_saved_request(
                &root,
                "Child",
                template(
                    "https://same.example",
                    "GET",
                    "throw new Error('script must not run')",
                    "",
                ),
            )
            .unwrap();
        let namespace = RequestNamespaceCatalog::from_workspace(&workspace);
        let executor = Denied(Mutex::new(Vec::new()));
        let run = run_chain_with_executor(
            &workspace,
            None,
            &[schedule(&child, "Root.Child")],
            &namespace,
            &ScriptCancellation::new(),
            None,
            &executor,
            ChainLimits::default(),
            &AtomicUsize::new(0),
        )
        .await;
        assert_eq!(*executor.0.lock().unwrap(), vec![child]);
        let error = run.error.unwrap().message;
        assert!(error.contains("policy denied"), "{error}");
        assert!(!error.contains("script must not run"));
        assert!(run.history.is_empty());
    }

    /// Spawn a blocking loopback server that records each request's request line
    /// plus its Authorization token, then answers 200.
    fn loopback_server(requests: usize) -> (String, u16, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().unwrap();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let recorded_worker = Arc::clone(&recorded);
        std::thread::spawn(move || {
            let mut handled = 0;
            while handled < requests {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let mut buffer = [0_u8; 8192];
                let mut header = String::new();
                loop {
                    match stream.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => header.push_str(&String::from_utf8_lossy(&buffer[..n])),
                        Err(_) => break,
                    }
                    if header.contains("\r\n\r\n") {
                        break;
                    }
                }
                let first_line = header.lines().next().unwrap_or_default().to_owned();
                let auth = header
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|line| {
                        line.split(':')
                            .nth(1)
                            .map(str::trim)
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .unwrap_or_default();
                recorded_worker
                    .lock()
                    .unwrap()
                    .push(format!("{first_line} | {auth}"));
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\n{}",
                );
                handled += 1;
            }
        });
        (address.ip().to_string(), address.port(), recorded)
    }

    fn schedule(id: &str, path: &str) -> ChainedRequest {
        ChainedRequest {
            id: id.to_owned(),
            path: path.to_owned(),
        }
    }

    #[tokio::test]
    async fn chained_prerequisite_token_reaches_parent_request_header() {
        let (host, port, recorded) = loopback_server(2);
        let base = format!("http://{host}:{port}");
        let mut workspace = Workspace::default();
        let auth = workspace.create_collection("Auth").unwrap();
        workspace
            .create_saved_request(
                &auth,
                "Refresh",
                template(
                    &format!("{base}/refresh"),
                    "POST",
                    "",
                    r#"api.environment.set("AUTH_TOKEN", "token-123");"#,
                ),
            )
            .unwrap();
        let mut me_draft = RequestDraft::new("GET", &format!("{base}/me"));
        me_draft.headers = vec![HeaderEntry::new("Authorization", "Bearer {{AUTH_TOKEN}}")];
        let me_template = RequestTemplate {
            request: me_draft,
            scripts: RequestScripts {
                pre_request: "api.requests.execute(Auth.Refresh);".to_owned(),
                post_response: String::new(),
            },
            documentation: String::new(),
            websocket: None,
        };
        let me_id = workspace
            .create_saved_request(&auth, "Me", me_template)
            .unwrap();
        let env_id = workspace.create_environment("Dev").unwrap();
        workspace.set_active_environment(Some(&env_id)).unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);

        let client = crate::core::request::build_client().unwrap();
        let sender = move |request: RequestDraft| {
            let client = client.clone();
            async move { crate::core::request::send_request(&client, request).await }
        };
        let cancellation = ScriptCancellation::new();
        let budget = AtomicUsize::new(0);

        // Parent Me's pre-request script chains Auth.Refresh, whose post-script
        // stores AUTH_TOKEN; Me then resolves its Authorization header from it.
        let run = run_chain(
            &workspace,
            Some(&env_id),
            &[schedule(&me_id, "Auth.Me")],
            &catalog,
            &cancellation,
            None,
            sender,
            ChainLimits::default(),
            &budget,
        )
        .await;

        assert!(
            run.error.is_none(),
            "unexpected chain error: {:?}",
            run.error
        );
        // Wait for the loopback server to finish answering.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if recorded.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let lines = recorded.lock().unwrap().clone();
        assert_eq!(
            lines.len(),
            2,
            "expected refresh + me on the wire: {lines:?}"
        );
        assert!(
            lines[0].contains("/refresh"),
            "first request should be refresh: {lines:?}"
        );
        let me_line = lines
            .iter()
            .find(|line| line.contains("/me "))
            .expect("me on the wire");
        assert!(
            me_line.contains("Bearer token-123"),
            "parent must receive the token produced by the chained prereq: {me_line}"
        );
        assert_eq!(run.history.len(), 2);
    }

    #[tokio::test]
    async fn recursive_chaining_runs_each_requests_own_scripts() {
        let (host, port, recorded) = loopback_server(2);
        let base = format!("http://{host}:{port}");
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("C").unwrap();
        // A -> (pre) B ; B post sets marker -> not needed; simply chain A -> B.
        let b_id = workspace
            .create_saved_request(&col, "B", template(&format!("{base}/b"), "GET", "", ""))
            .unwrap();
        let a_id = workspace
            .create_saved_request(
                &col,
                "A",
                template(
                    &format!("{base}/a"),
                    "GET",
                    "api.requests.execute(C.B);",
                    "",
                ),
            )
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let client = crate::core::request::build_client().unwrap();
        let sender = move |request: RequestDraft| {
            let client = client.clone();
            async move { crate::core::request::send_request(&client, request).await }
        };
        let run = run_chain(
            &workspace,
            None,
            &[schedule(&a_id, "C.A")],
            &catalog,
            &ScriptCancellation::new(),
            None,
            sender,
            ChainLimits::default(),
            &AtomicUsize::new(0),
        )
        .await;
        assert!(run.error.is_none(), "unexpected: {:?}", run.error);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if !recorded.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // A executes, then recursively B executes (nested before A's network).
        assert!(
            recorded.lock().unwrap().len() >= 2,
            "expected A and its nested B on the wire: {:?}",
            recorded.lock().unwrap()
        );
        let lines = recorded.lock().unwrap().clone();
        assert!(
            lines.iter().any(|line| line.contains("/b ")),
            "nested B should have run: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("/a ")),
            "A should have run after its nested chain: {lines:?}"
        );
        let _ = b_id;
    }

    async fn run_with_limits(
        workspace: &Workspace,
        catalog: &RequestNamespaceCatalog,
        scheduled: Vec<ChainedRequest>,
        limits: ChainLimits,
    ) -> ChainRun {
        let client = crate::core::request::build_client().unwrap();
        let sender = move |request: RequestDraft| {
            let client = client.clone();
            async move { crate::core::request::send_request(&client, request).await }
        };
        run_chain(
            workspace,
            None,
            &scheduled,
            catalog,
            &ScriptCancellation::new(),
            None,
            sender,
            limits,
            &AtomicUsize::new(0),
        )
        .await
    }

    #[tokio::test]
    async fn cycle_is_detected_and_reported() {
        let host = "127.0.0.1";
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("C").unwrap();
        let a = workspace
            .create_saved_request(
                &col,
                "A",
                template(
                    &format!("http://{host}:1/a"),
                    "GET",
                    "api.requests.execute(C.B);",
                    "",
                ),
            )
            .unwrap();
        workspace
            .create_saved_request(
                &col,
                "B",
                template(
                    &format!("http://{host}:1/b"),
                    "GET",
                    "api.requests.execute(C.C);",
                    "",
                ),
            )
            .unwrap();
        workspace
            .create_saved_request(
                &col,
                "C",
                template(
                    &format!("http://{host}:1/c"),
                    "GET",
                    "api.requests.execute(C.A);",
                    "",
                ),
            )
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let run = run_with_limits(
            &workspace,
            &catalog,
            vec![schedule(&a, "C.A")],
            ChainLimits::default(),
        )
        .await;
        let error = run.error.expect("cycle must be detected");
        assert!(error.message.contains("cycle"), "{}", error.message);
    }

    #[tokio::test]
    async fn total_execution_limit_is_enforced() {
        let (host, port, recorded) = loopback_server(2);
        let base = format!("http://{host}:{port}");
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("C").unwrap();
        let r = workspace
            .create_saved_request(&col, "R", template(&format!("{base}/r"), "GET", "", ""))
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let mut scheduled = Vec::new();
        for _ in 0..3 {
            scheduled.push(schedule(&r, "C.R"));
        }
        let limits = ChainLimits {
            max_depth: 5,
            max_total: 2,
            ..Default::default()
        };
        let run = run_with_limits(&workspace, &catalog, scheduled, limits).await;
        let error = run.error.expect("total limit must trip");
        assert!(error.message.contains("total"), "{}", error.message);
        // Two executed before the limit; the third was rejected.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if recorded.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(recorded.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn nesting_depth_limit_is_enforced() {
        let host = "127.0.0.1";
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("C").unwrap();
        let x1 = workspace
            .create_saved_request(
                &col,
                "X1",
                template(
                    &format!("http://{host}:1/x1"),
                    "GET",
                    "api.requests.execute(C.X2);",
                    "",
                ),
            )
            .unwrap();
        workspace
            .create_saved_request(
                &col,
                "X2",
                template(
                    &format!("http://{host}:1/x2"),
                    "GET",
                    "api.requests.execute(C.X3);",
                    "",
                ),
            )
            .unwrap();
        workspace
            .create_saved_request(
                &col,
                "X3",
                template(&format!("http://{host}:1/x3"), "GET", "", ""),
            )
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let limits = ChainLimits {
            max_depth: 2,
            max_total: 10,
            ..Default::default()
        };
        let run = run_with_limits(&workspace, &catalog, vec![schedule(&x1, "C.X1")], limits).await;
        let error = run.error.expect("depth limit must trip");
        assert!(error.message.contains("depth"), "{}", error.message);
    }

    #[tokio::test]
    async fn stale_reference_reports_clearly() {
        let host = "127.0.0.1";
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("C").unwrap();
        let r = workspace
            .create_saved_request(
                &col,
                "R",
                template(&format!("http://{host}:1/r"), "GET", "", ""),
            )
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let run = run_with_limits(
            &workspace,
            &catalog,
            vec![schedule("missing-id", "C.Gone")],
            ChainLimits::default(),
        )
        .await;
        let error = run.error.expect("stale ref must fail");
        assert!(
            error.message.contains("no longer present"),
            "{}",
            error.message
        );
        let _ = r;
    }

    #[tokio::test]
    async fn pre_cancelled_chain_refuses_to_start() {
        let host = "127.0.0.1";
        let mut workspace = Workspace::default();
        let col = workspace.create_collection("C").unwrap();
        let r = workspace
            .create_saved_request(
                &col,
                "R",
                template(&format!("http://{host}:1/r"), "GET", "", ""),
            )
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);
        let client = crate::core::request::build_client().unwrap();
        let sender = move |request: RequestDraft| {
            let client = client.clone();
            async move { crate::core::request::send_request(&client, request).await }
        };
        let cancellation = ScriptCancellation::new();
        cancellation.cancel();
        let run = run_chain(
            &workspace,
            None,
            &[schedule(&r, "C.R")],
            &catalog,
            &cancellation,
            None,
            sender,
            ChainLimits::default(),
            &AtomicUsize::new(0),
        )
        .await;
        assert!(run.error.is_some());
        assert!(run.error.unwrap().message.contains("cancelled"));
    }

    #[tokio::test]
    async fn chained_request_own_script_can_await_execute() {
        // A chained request's own post-response script may itself `await
        // api.requests.execute(...)`: run_chain threads the inline chainer into
        // the chained scripts, so the nested await settles instead of hanging
        // with "script finished without settling its top-level promise".
        let (host, port, _recorded) = loopback_server(1);
        let base = format!("http://{host}:{port}");
        let mut workspace = Workspace::default();
        let env_id = workspace.create_environment("Dev").unwrap();
        workspace.set_active_environment(Some(&env_id)).unwrap();
        let col = workspace.create_collection("C").unwrap();
        let a = workspace
            .create_saved_request(&col, "A", template(&format!("{base}/a"), "GET", "", ""))
            .unwrap();
        let b = workspace
            .create_saved_request(
                &col,
                "B",
                template(
                    &format!("{base}/b"),
                    "GET",
                    "",
                    "await api.requests.execute(C.A); api.environment.set(\"after\", \"yes\");",
                ),
            )
            .unwrap();
        let catalog = RequestNamespaceCatalog::from_workspace(&workspace);

        let client = crate::core::request::build_client().unwrap();
        let sender = move |request: RequestDraft| {
            let client = client.clone();
            async move { crate::core::request::send_request(&client, request).await }
        };

        let called = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe = called.clone();
        let chainer = move |requested: &[ChainedRequest]| -> Vec<Result<ChainRun, String>> {
            probe.fetch_add(requested.len(), std::sync::atomic::Ordering::SeqCst);
            vec![Ok(ChainRun::default()); requested.len()]
        };

        let run = run_chain(
            &workspace,
            Some(&env_id),
            &[schedule(&b, "C.B")],
            &catalog,
            &ScriptCancellation::new(),
            Some(&chainer),
            sender,
            ChainLimits::default(),
            &AtomicUsize::new(0),
        )
        .await;

        assert!(
            run.error.is_none(),
            "unexpected chain error: {:?}",
            run.error
        );
        // The chainer was threaded into B's own post-response script, so its
        // nested `await execute(C.A)` resolved (called once) and the script ran
        // past it, rather than hanging with "never resolved".
        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            run.environment_mutations
                .contains(&EnvironmentMutation::Set {
                    key: "after".to_owned(),
                    value: "yes".to_owned(),
                }),
            "mutations: {:?}",
            run.environment_mutations
        );
        let _ = a;
    }
}
