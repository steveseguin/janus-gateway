//! Session and handle management.
//!
//! A **session** represents a client connection (e.g. a browser tab). Each
//! session can have multiple **handles**, each attached to a different plugin.

use dashmap::DashMap;
use janus_plugin_api::{HandleId, PluginSession, SessionId};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Notify;
use tracing::{debug, warn};

/// Manages all active sessions and their handles.
pub struct SessionManager {
    sessions: DashMap<SessionId, Session>,
    /// Timeout in seconds. 0 = no timeout.
    session_timeout: u64,
    /// Notified when a new session is created (for watchdog wakeup).
    notify: Arc<Notify>,
}

/// A single client session.
#[derive(Debug)]
pub struct Session {
    pub id: SessionId,
    /// Plugin handles attached to this session.
    pub handles: DashMap<HandleId, HandleInfo>,
    /// Last activity timestamp.
    pub last_activity: Instant,
    /// Whether this session has been marked for destruction.
    pub destroying: bool,
}

/// A plugin handle within a session.
#[derive(Debug, Clone)]
pub struct HandleInfo {
    pub handle_id: HandleId,
    pub session_id: SessionId,
    /// The plugin package name this handle is attached to.
    pub plugin_package: String,
}

impl HandleInfo {
    /// Create a [`PluginSession`] for passing to the plugin API.
    pub fn plugin_session(&self) -> PluginSession {
        PluginSession::new(self.session_id, self.handle_id)
    }
}

impl SessionManager {
    /// Create a new session manager with the given timeout (seconds).
    pub fn new(session_timeout: u64) -> Self {
        Self {
            sessions: DashMap::new(),
            session_timeout,
            notify: Arc::new(Notify::new()),
        }
    }

    /// Create a new session with a random ID and return it.
    pub fn create_session(&self) -> SessionId {
        let id = SessionId::random();
        let session = Session {
            id,
            handles: DashMap::new(),
            last_activity: Instant::now(),
            destroying: false,
        };
        self.sessions.insert(id, session);
        self.notify.notify_one();
        debug!(session_id = %id, "session created");
        id
    }

    /// Create a session with a specific ID (e.g. for reclaiming).
    pub fn create_session_with_id(&self, id: SessionId) -> bool {
        if self.sessions.contains_key(&id) {
            return false;
        }
        let session = Session {
            id,
            handles: DashMap::new(),
            last_activity: Instant::now(),
            destroying: false,
        };
        self.sessions.insert(id, session);
        self.notify.notify_one();
        debug!(session_id = %id, "session created (explicit ID)");
        true
    }

    /// Look up a session. Returns None if not found or marked for destruction.
    pub fn get_session(&self, id: SessionId) -> Option<dashmap::mapref::one::Ref<'_, SessionId, Session>> {
        self.sessions.get(&id).filter(|s| !s.destroying)
    }

    /// Touch a session to reset its timeout.
    pub fn touch_session(&self, id: SessionId) -> bool {
        if let Some(mut session) = self.sessions.get_mut(&id) {
            session.last_activity = Instant::now();
            true
        } else {
            false
        }
    }

    /// Attach a plugin handle to a session. Returns the handle ID.
    pub fn attach_handle(
        &self,
        session_id: SessionId,
        plugin_package: String,
    ) -> crate::Result<HandleId> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(crate::Error::SessionNotFound(session_id.0))?;

        let handle_id = HandleId::random();
        let handle_info = HandleInfo {
            handle_id,
            session_id,
            plugin_package,
        };
        session.handles.insert(handle_id, handle_info);
        debug!(session_id = %session_id, handle_id = %handle_id, "handle attached");
        Ok(handle_id)
    }

    /// Look up a handle within a session.
    pub fn get_handle(
        &self,
        session_id: SessionId,
        handle_id: HandleId,
    ) -> crate::Result<HandleInfo> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(crate::Error::SessionNotFound(session_id.0))?;
        session
            .handles
            .get(&handle_id)
            .map(|h| h.clone())
            .ok_or(crate::Error::HandleNotFound(handle_id.0))
    }

    /// Detach a handle from a session.
    pub fn detach_handle(
        &self,
        session_id: SessionId,
        handle_id: HandleId,
    ) -> crate::Result<HandleInfo> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(crate::Error::SessionNotFound(session_id.0))?;
        session
            .handles
            .remove(&handle_id)
            .map(|(_, info)| info)
            .ok_or(crate::Error::HandleNotFound(handle_id.0))
    }

    /// Mark a session for destruction and remove it. Returns all handle infos.
    pub fn destroy_session(&self, id: SessionId) -> crate::Result<Vec<HandleInfo>> {
        let (_, session) = self
            .sessions
            .remove(&id)
            .ok_or(crate::Error::SessionNotFound(id.0))?;
        let handles: Vec<HandleInfo> = session
            .handles
            .into_iter()
            .map(|(_, info)| info)
            .collect();
        debug!(session_id = %id, handles = handles.len(), "session destroyed");
        Ok(handles)
    }

    /// Return the number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Return IDs of all active sessions.
    pub fn session_ids(&self) -> Vec<SessionId> {
        self.sessions.iter().map(|r| *r.key()).collect()
    }

    /// Collect sessions that have timed out. Returns their IDs.
    pub fn collect_timed_out(&self) -> Vec<SessionId> {
        if self.session_timeout == 0 {
            return Vec::new();
        }
        let timeout = std::time::Duration::from_secs(self.session_timeout);
        let now = Instant::now();
        self.sessions
            .iter()
            .filter(|r| !r.destroying && now.duration_since(r.last_activity) > timeout)
            .map(|r| *r.key())
            .collect()
    }

    /// Start the session watchdog task that periodically checks for timeouts.
    /// Returns a handle to the spawned task.
    pub fn start_watchdog(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                // Sleep for half the timeout period (or 5s if no timeout).
                let sleep_secs = if mgr.session_timeout > 0 {
                    mgr.session_timeout / 2
                } else {
                    5
                };
                tokio::time::sleep(std::time::Duration::from_secs(sleep_secs.max(1))).await;

                let timed_out = mgr.collect_timed_out();
                for id in timed_out {
                    warn!(session_id = %id, "session timed out");
                    let _ = mgr.destroy_session(id);
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_get_session() {
        let mgr = SessionManager::new(60);
        let id = mgr.create_session();
        assert!(mgr.get_session(id).is_some());
        assert_eq!(mgr.session_count(), 1);
    }

    #[test]
    fn create_session_with_explicit_id() {
        let mgr = SessionManager::new(60);
        let id = SessionId(42);
        assert!(mgr.create_session_with_id(id));
        assert!(mgr.get_session(id).is_some());
        // Duplicate returns false
        assert!(!mgr.create_session_with_id(id));
    }

    #[test]
    fn destroy_session_returns_handles() {
        let mgr = SessionManager::new(60);
        let sid = mgr.create_session();
        let h1 = mgr.attach_handle(sid, "janus.plugin.echotest".into()).unwrap();
        let h2 = mgr.attach_handle(sid, "janus.plugin.videoroom".into()).unwrap();
        let handles = mgr.destroy_session(sid).unwrap();
        assert_eq!(handles.len(), 2);
        let ids: Vec<HandleId> = handles.iter().map(|h| h.handle_id).collect();
        assert!(ids.contains(&h1));
        assert!(ids.contains(&h2));
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn destroy_nonexistent_session_errors() {
        let mgr = SessionManager::new(60);
        assert!(mgr.destroy_session(SessionId(999)).is_err());
    }

    #[test]
    fn attach_and_get_handle() {
        let mgr = SessionManager::new(60);
        let sid = mgr.create_session();
        let hid = mgr.attach_handle(sid, "janus.plugin.echotest".into()).unwrap();
        let info = mgr.get_handle(sid, hid).unwrap();
        assert_eq!(info.plugin_package, "janus.plugin.echotest");
        assert_eq!(info.session_id, sid);
    }

    #[test]
    fn attach_to_nonexistent_session_errors() {
        let mgr = SessionManager::new(60);
        assert!(mgr.attach_handle(SessionId(999), "test".into()).is_err());
    }

    #[test]
    fn detach_handle() {
        let mgr = SessionManager::new(60);
        let sid = mgr.create_session();
        let hid = mgr.attach_handle(sid, "test.plugin".into()).unwrap();
        let info = mgr.detach_handle(sid, hid).unwrap();
        assert_eq!(info.handle_id, hid);
        // Second detach fails
        assert!(mgr.detach_handle(sid, hid).is_err());
    }

    #[test]
    fn touch_session_updates_activity() {
        let mgr = SessionManager::new(60);
        let sid = mgr.create_session();
        let before = mgr.get_session(sid).unwrap().last_activity;
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(mgr.touch_session(sid));
        let after = mgr.get_session(sid).unwrap().last_activity;
        assert!(after > before);
    }

    #[test]
    fn touch_nonexistent_session_returns_false() {
        let mgr = SessionManager::new(60);
        assert!(!mgr.touch_session(SessionId(999)));
    }

    #[test]
    fn session_ids_returns_all() {
        let mgr = SessionManager::new(60);
        let s1 = mgr.create_session();
        let s2 = mgr.create_session();
        let ids = mgr.session_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&s1));
        assert!(ids.contains(&s2));
    }

    #[test]
    fn collect_timed_out_with_zero_timeout() {
        let mgr = SessionManager::new(0);
        mgr.create_session();
        assert!(mgr.collect_timed_out().is_empty());
    }

    #[test]
    fn collect_timed_out_finds_expired() {
        let mgr = SessionManager::new(1); // 1 second timeout
        let sid = mgr.create_session();
        // Manually backdate the activity
        if let Some(mut session) = mgr.sessions.get_mut(&sid) {
            session.last_activity = Instant::now() - std::time::Duration::from_secs(5);
        }
        let timed_out = mgr.collect_timed_out();
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0], sid);
    }

    #[test]
    fn handle_info_plugin_session() {
        let info = HandleInfo {
            handle_id: HandleId(1),
            session_id: SessionId(2),
            plugin_package: "test".into(),
        };
        let ps = info.plugin_session();
        assert_eq!(ps.session_id, SessionId(2));
        assert_eq!(ps.handle_id, HandleId(1));
    }

    #[test]
    fn concurrent_session_creation() {
        use std::sync::Arc;
        use std::thread;

        let mgr = Arc::new(SessionManager::new(60));
        let mut handles = Vec::new();

        for _ in 0..10 {
            let mgr = Arc::clone(&mgr);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    mgr.create_session();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(mgr.session_count(), 1000);
    }
}
