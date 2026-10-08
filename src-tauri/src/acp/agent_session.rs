//! Per-session update routing on an ACP client connection.
//!
//! codeg sends `session/new`, `session/load` and `session/resume` itself —
//! partly untyped, so it can read fields the typed responses drop (see
//! `send_new_session_capturing_models`) — and then needs the session's updates
//! routed to it. The official runtime only routes updates for sessions it
//! started itself: `ConnectionTo::attach_session` is crate-private since
//! `agent-client-protocol` 2.0. This is the same routing that method installs —
//! a dynamic handler claiming every message from the agent that names this
//! session id, feeding an unbounded channel — minus the prompt helpers codeg
//! never used. Prompt responses enter the SAME queue as updates, retaining
//! their response router until the application has applied all earlier updates.
//!
//! Registering the handler is also what releases any updates the agent sent
//! BEFORE it existed: the `Agent` role's default handler parks every message
//! carrying a `sessionId` for retry, and parked messages are replayed into each
//! newly added dynamic handler. That is how a `session/load` replay, or the
//! `available_commands_update` an agent sends right after `session/new`, reaches
//! the session even though it raced the response.

use agent_client_protocol::schema::v1::{
    NewSessionResponse, PromptRequest, PromptResponse, RequestId, SessionId, SessionModeState,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{
    Agent, ConnectionTo, Dispatch, DynamicHandlerGuard, HandleDispatchFrom, Handled,
};
use futures::channel::mpsc;
use futures::{future::BoxFuture, FutureExt, StreamExt};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

type PromptResult = Result<PromptResponse, agent_client_protocol::Error>;

type PendingPrompts = Arc<Mutex<HashSet<RequestId>>>;

/// Also reclaim registrations for EOF/send failure or a dropped waiter, which
/// never pass through the session's wire-response handler.
struct PromptRegistration {
    id: RequestId,
    pending: PendingPrompts,
}

impl Drop for PromptRegistration {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

/// The response boundary is visible to the active turn reader, which must
/// stop reading when its own response is ready. An idle reader simply skips it.
pub enum SessionRead {
    Dispatch(Box<Dispatch>),
    PromptResponse(RequestId),
}

/// A session the connection routes updates to. Dropping it unregisters the
/// routing; updates for the session then fall back to the connection's other
/// handlers (and, having a `sessionId`, are parked for a future router).
#[derive(Debug)]
pub struct AgentSession {
    session_id: SessionId,
    modes: Option<SessionModeState>,
    connection: ConnectionTo<Agent>,
    update_rx: mpsc::UnboundedReceiver<Dispatch>,
    pending_prompts: PendingPrompts,
    _routing: DynamicHandlerGuard<Agent>,
}

impl AgentSession {
    /// Start routing the updates of the session `response` describes.
    pub fn attach(
        connection: &ConnectionTo<Agent>,
        response: NewSessionResponse,
    ) -> Result<Self, agent_client_protocol::Error> {
        let NewSessionResponse {
            session_id, modes, ..
        } = response;
        let (update_tx, update_rx) = mpsc::unbounded();
        let pending_prompts = PendingPrompts::default();
        let routing = connection.add_dynamic_handler(SessionRouter {
            session_id: session_id.clone(),
            update_tx,
            pending_prompts: pending_prompts.clone(),
        })?;
        Ok(Self {
            session_id,
            modes,
            connection: connection.clone(),
            update_rx,
            pending_prompts,
            _routing: routing,
        })
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The modes the agent reported when the session was created or loaded.
    pub fn modes(&self) -> &Option<SessionModeState> {
        &self.modes
    }

    /// The connection the session lives on (cheap to clone).
    pub fn connection(&self) -> ConnectionTo<Agent> {
        self.connection.clone()
    }

    /// Register this request while holding the same short lock the router
    /// checks. Even a response arriving on another executor thread before
    /// `send_request_to` returns cannot miss registration and bypass the FIFO.
    /// Nothing under the lock awaits traffic or application work.
    pub fn prompt(
        &mut self,
        request: PromptRequest,
    ) -> (RequestId, BoxFuture<'static, PromptResult>) {
        let (pending, id) = {
            let mut ids = self.pending_prompts.lock().unwrap();
            let pending = self.connection.send_request_to(Agent, request);
            let id = pending.id().clone();
            ids.insert(id.clone());
            (pending, id)
        };
        let registration = PromptRegistration {
            id: id.clone(),
            pending: self.pending_prompts.clone(),
        };
        let wait = async move {
            let _registration = registration;
            pending.block_task().await
        };
        (id, wait.boxed())
    }

    /// The next message the agent sent about this session.
    ///
    /// Waiting here never times out on its own: the router outlives an agent
    /// that has gone quiet (the runtime keeps its handlers for as long as any
    /// `ConnectionTo` exists, and this session holds one). So an `Err` does not
    /// mean one unreadable message — it means the router itself is gone, i.e.
    /// the connection is, and a caller must stop reading rather than retry.
    pub async fn read_update(&mut self) -> Result<Dispatch, agent_client_protocol::Error> {
        loop {
            if let SessionRead::Dispatch(dispatch) = self.read_update_or_response().await? {
                return Ok(*dispatch);
            }
        }
    }

    pub async fn read_update_or_response(
        &mut self,
    ) -> Result<SessionRead, agent_client_protocol::Error> {
        let update = self.update_rx.next().await.ok_or_else(|| {
            agent_client_protocol::util::internal_error("session channel closed unexpectedly")
        })?;
        Ok(match update {
            Dispatch::Response(result, router) => {
                let id = router.id().clone();
                router.route_with_result(result)?;
                SessionRead::PromptResponse(id)
            }
            dispatch => SessionRead::Dispatch(Box::new(dispatch)),
        })
    }
}

/// Claims every message from the agent whose `params.sessionId` names this
/// session. Mirrors the runtime's own `ActiveSessionHandler`.
struct SessionRouter {
    session_id: SessionId,
    update_tx: mpsc::UnboundedSender<Dispatch>,
    pending_prompts: PendingPrompts,
}

impl HandleDispatchFrom<Agent> for SessionRouter {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<Agent>,
    ) -> Result<Handled<Dispatch>, agent_client_protocol::Error> {
        MatchDispatchFrom::new(message, &cx)
            .if_dispatch_from(Agent, async |message: Dispatch| {
                let prompt_response = match &message {
                    Dispatch::Response(_, router) => {
                        self.pending_prompts.lock().unwrap().remove(router.id())
                    }
                    _ => false,
                };
                if prompt_response || dispatch_session_id(&message) == Some(&*self.session_id.0) {
                    self.update_tx
                        .unbounded_send(message)
                        .map_err(agent_client_protocol::util::internal_error)?;
                    return Ok(Handled::Yes);
                }
                Ok(Handled::No {
                    message,
                    retry: false,
                })
            })
            .await
            .done()
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        format!("AgentSession({})", self.session_id)
    }
}

/// The `sessionId` a request or notification names, if it names one as a
/// string. A `null` (or otherwise non-string) id matches no session — the
/// runtime's own router errors on it instead, which is why codeg claims the
/// `null` ones, the shape agents actually send, before they get this far
/// (`ClaimNullSessionIds`).
fn dispatch_session_id(dispatch: &Dispatch) -> Option<&str> {
    let message = match dispatch {
        Dispatch::Request(message, _) | Dispatch::Notification(message) => message,
        Dispatch::Response(..) => return None,
    };
    message.params().get("sessionId")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{
        AvailableCommandsUpdate, NewSessionRequest, SessionNotification, SessionUpdate,
    };
    use agent_client_protocol::{Channel, Client, Responder, UntypedMessage};

    #[derive(
        Debug,
        Clone,
        serde::Serialize,
        serde::Deserialize,
        agent_client_protocol::JsonRpcNotification,
    )]
    #[notification(method = "test/after_response")]
    struct AfterResponse {}

    /// Pause application consumption until the actual ACP reader has routed
    /// the response and a later wire frame. No scheduler timing or sleeps:
    /// the response must still wait for every earlier application update.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prompt_response_waits_for_earlier_session_updates() {
        assert_prompt_response_boundary(false).await;
    }

    #[tokio::test]
    async fn rejected_prompt_waits_for_earlier_session_updates() {
        assert_prompt_response_boundary(true).await;
    }

    async fn assert_prompt_response_boundary(rejected: bool) {
        use agent_client_protocol::schema::v1::StopReason;
        let (client_end, agent_end) = Channel::duplex();
        let received = std::sync::Arc::new(tokio::sync::Notify::new());
        let signal = received.clone();
        let agent = tokio::spawn(
            Agent
                .builder()
                .on_receive_request(
                    async move |_req: PromptRequest,
                                responder: Responder<PromptResponse>,
                                cx: ConnectionTo<Client>| {
                        for _ in 0..8 {
                            cx.send_notification(commands_update("s1"))?;
                        }
                        if rejected {
                            responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params(),
                            )?;
                        } else {
                            responder.respond(PromptResponse::new(StopReason::EndTurn))?;
                        }
                        // A later update must stay beyond the turn boundary,
                        // even when it is already buffered along with the rest.
                        cx.send_notification(commands_update("s1"))?;
                        cx.send_notification(AfterResponse {})
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(agent_end),
        );
        let result = Client
            .builder()
            .on_receive_notification(
                async move |_msg: AfterResponse, _cx: ConnectionTo<Agent>| {
                    signal.notify_one();
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(client_end, async move |cx: ConnectionTo<Agent>| {
                let mut session =
                    AgentSession::attach(&cx, NewSessionResponse::new(SessionId::new("s1")))?;
                let (id, mut response) =
                    session.prompt(PromptRequest::new(SessionId::new("s1"), Vec::new()));
                tokio::time::timeout(std::time::Duration::from_secs(5), received.notified())
                    .await
                    .expect("reader reached the frame AFTER the prompt response");
                for _ in 0..8 {
                    assert!(
                        futures::poll!(response.as_mut()).is_pending(),
                        "prompt response overtook an unprocessed session update"
                    );
                    assert_eq!(update_session_id(session.read_update().await?), "s1");
                }
                assert!(matches!(
                    session.read_update_or_response().await?,
                    SessionRead::PromptResponse(received_id) if received_id == id
                ));
                let result = response.await;
                if rejected {
                    assert_eq!(
                        result.unwrap_err().code,
                        agent_client_protocol::Error::invalid_params().code
                    );
                } else {
                    assert_eq!(result?.stop_reason, StopReason::EndTurn);
                }
                assert_eq!(update_session_id(session.read_update().await?), "s1");
                Ok(())
            })
            .await;
        agent.abort();
        result.expect("client connection");
    }

    fn notification(params: serde_json::Value) -> Dispatch {
        Dispatch::Notification(UntypedMessage::new("session/update", params).unwrap())
    }

    #[tokio::test]
    async fn retired_prompt_marker_does_not_release_new_prompt_and_drop_releases_waiter() {
        use agent_client_protocol::schema::v1::StopReason;
        let (client_end, agent_end) = Channel::duplex();
        let received = std::sync::Arc::new(tokio::sync::Notify::new());
        let signal = received.clone();
        let agent = tokio::spawn(
            Agent
                .builder()
                .on_receive_request(
                    async |_req: PromptRequest,
                           responder: Responder<PromptResponse>,
                           cx: ConnectionTo<Client>| {
                        responder.respond(PromptResponse::new(StopReason::Cancelled))?;
                        cx.send_notification(AfterResponse {})
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(agent_end),
        );
        let result = Client.builder().on_receive_notification(
            async move |_msg: AfterResponse, _cx: ConnectionTo<Agent>| {
                signal.notify_one();
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        ).connect_with(client_end, async move |cx: ConnectionTo<Agent>| {
            let mut session = AgentSession::attach(&cx, NewSessionResponse::new(SessionId::new("s1")))?;
            let request = || PromptRequest::new(SessionId::new("s1"), Vec::new());
            let (old_id, old_response) = session.prompt(request());
            received.notified().await;
            // Exactly the cancellation path's ownership transfer: the old
            // response remains awaited while the next turn is admitted.
            let background = tokio::spawn(old_response);
            let (new_id, mut new_response) = session.prompt(request());
            received.notified().await;
            assert_ne!(old_id, new_id);
            assert!(matches!(session.read_update_or_response().await?, SessionRead::PromptResponse(id) if id == old_id));
            assert_eq!(background.await.unwrap()?.stop_reason, StopReason::Cancelled);
            assert!(futures::poll!(new_response.as_mut()).is_pending());
            // Idle readers also retire markers without emitting an update.
            assert!(session.read_update().now_or_never().is_none());
            assert_eq!(new_response.await?.stop_reason, StopReason::Cancelled);

            let (_, orphan) = session.prompt(request());
            received.notified().await;
            drop(session);
            assert!(orphan.await.unwrap_err().to_string().contains("never received"));
            Ok(())
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), result).await;
        agent.abort();
        result
            .expect("no marker waiter may hang")
            .expect("client connection");
    }

    fn commands_update(session_id: &str) -> SessionNotification {
        SessionNotification::new(
            SessionId::new(session_id),
            SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(Vec::new())),
        )
    }

    fn update_session_id(dispatch: Dispatch) -> String {
        dispatch_session_id(&dispatch)
            .expect("a routed update names its session")
            .to_string()
    }

    /// The whole contract, against the real runtime over an in-memory pipe:
    /// codeg creates the session UNTYPED (as `send_new_session_capturing_models`
    /// does), attaches afterwards, and still receives the update the agent sent
    /// BEFORE its `session/new` response — parked by the runtime because it
    /// carries a `sessionId`, and replayed into the router the moment it is
    /// added. That replay is what a `session/load` history drain and the
    /// `available_commands_update` agents send right after `session/new` depend
    /// on. A frame for another session must not leak in.
    #[tokio::test]
    async fn an_attached_session_receives_updates_the_agent_sent_before_the_response() {
        let (client_end, agent_end) = Channel::duplex();

        let agent = tokio::spawn(
            Agent
                .builder()
                .on_receive_request(
                    async |_req: NewSessionRequest,
                           responder: Responder<NewSessionResponse>,
                           cx: ConnectionTo<Client>| {
                        // Racing the response: sent first, so it reaches the
                        // client before any router for `s1` exists.
                        cx.send_notification(commands_update("s1"))?;
                        cx.send_notification(commands_update("someone-else"))?;
                        responder.respond(NewSessionResponse::new(SessionId::new("s1")))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(agent_end),
        );

        let routed = Client
            .builder()
            .connect_with(client_end, async |cx: ConnectionTo<Agent>| {
                let raw = cx
                    .send_request_to(
                        Agent,
                        UntypedMessage::new("session/new", NewSessionRequest::new("/tmp"))?,
                    )
                    .block_task()
                    .await?;
                let response: NewSessionResponse = serde_json::from_value(raw)
                    .map_err(agent_client_protocol::Error::into_internal_error)?;
                let mut session = AgentSession::attach(&cx, response)?;
                assert_eq!(&*session.session_id().0, "s1");

                let first =
                    tokio::time::timeout(std::time::Duration::from_secs(5), session.read_update())
                        .await
                        .expect("the parked update is replayed into the new router")?;
                // Nothing else is routed here: the other session's frame stays
                // parked for a router of its own.
                let nothing_more = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    session.read_update(),
                )
                .await;
                assert!(nothing_more.is_err(), "another session's update leaked in");
                Ok(update_session_id(first))
            })
            .await
            .expect("client connection");

        assert_eq!(routed, "s1");
        agent.abort();
    }

    #[test]
    fn a_dispatch_names_its_session_only_through_a_string_id() {
        assert_eq!(
            dispatch_session_id(&notification(serde_json::json!({"sessionId": "s1"}))),
            Some("s1")
        );
        assert_eq!(
            dispatch_session_id(&notification(serde_json::json!({"sessionId": null}))),
            None
        );
        assert_eq!(
            dispatch_session_id(&notification(serde_json::json!({"update": {}}))),
            None
        );
    }
}
