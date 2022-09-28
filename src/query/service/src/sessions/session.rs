// Copyright 2021 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::net::SocketAddr;
use std::sync::Arc;

use chrono_tz::Tz;
use common_exception::ErrorCode;
use common_exception::Result;
use common_io::prelude::FormatSettings;
use common_meta_types::GrantObject;
use common_meta_types::UserInfo;
use common_meta_types::UserPrivilegeType;
use common_users::RoleCacheManager;
use futures::channel::*;
use parking_lot::RwLock;

use crate::clusters::ClusterDiscovery;
use crate::servers::http::v1::HttpQueryManager;
use crate::sessions::QueryContext;
use crate::sessions::QueryContextShared;
use crate::sessions::SessionContext;
use crate::sessions::SessionManager;
use crate::sessions::SessionStatus;
use crate::sessions::SessionType;
use crate::sessions::Settings;
use crate::Config;

pub struct Session {
    pub(in crate::sessions) id: String,
    pub(in crate::sessions) typ: RwLock<SessionType>,
    pub(in crate::sessions) session_ctx: Arc<SessionContext>,
    status: Arc<RwLock<SessionStatus>>,
    pub(in crate::sessions) mysql_connection_id: Option<u32>,
}

impl Session {
    pub fn try_create(
        id: String,
        typ: SessionType,
        session_ctx: Arc<SessionContext>,
        mysql_connection_id: Option<u32>,
    ) -> Result<Arc<Session>> {
        let status = Arc::new(Default::default());
        Ok(Arc::new(Session {
            id,
            typ: RwLock::new(typ),
            status,
            session_ctx,
            mysql_connection_id,
        }))
    }

    pub fn get_mysql_conn_id(self: &Arc<Self>) -> Option<u32> {
        self.mysql_connection_id
    }

    pub fn get_id(self: &Arc<Self>) -> String {
        self.id.clone()
    }

    pub fn get_type(&self) -> SessionType {
        let lock = self.typ.read();
        lock.clone()
    }

    pub fn set_type(&self, typ: SessionType) {
        let mut lock = self.typ.write();
        *lock = typ;
    }

    pub fn is_aborting(self: &Arc<Self>) -> bool {
        self.session_ctx.get_abort()
    }

    pub fn quit(self: &Arc<Self>) {
        let session_ctx = self.session_ctx.clone();
        if session_ctx.get_current_query_id().is_some() {
            if let Some(io_shutdown) = session_ctx.take_io_shutdown_tx() {
                let (tx, rx) = oneshot::channel();
                if io_shutdown.send(tx).is_ok() {
                    // We ignore this error because the receiver is return cancelled error.
                    let _ = futures::executor::block_on(rx);
                }
            }
        }

        let http_queries_manager = HttpQueryManager::instance();
        http_queries_manager.kill_session(&self.id);
    }

    pub fn kill(self: &Arc<Self>) {
        self.session_ctx.set_abort(true);
        self.quit();
    }

    pub fn force_kill_session(self: &Arc<Self>) {
        self.force_kill_query(ErrorCode::AbortedQuery(
            "Aborted query, because the server is shutting down or the query was killed",
        ));
        self.kill(/* shutdown io stream */);
    }

    pub fn force_kill_query(self: &Arc<Self>, cause: ErrorCode) {
        let session_ctx = self.session_ctx.clone();

        if let Some(context_shared) = session_ctx.get_query_context_shared() {
            context_shared.kill(cause);
        }
    }

    /// Create a query context for query.
    /// For a query, execution environment(e.g cluster) should be immutable.
    /// We can bind the environment to the context in create_context method.
    pub async fn create_query_context(self: &Arc<Self>) -> Result<Arc<QueryContext>> {
        let config = self.get_config();
        let session = self.clone();
        let cluster = ClusterDiscovery::instance().discover(&config).await?;
        let shared = QueryContextShared::try_create(config, session, cluster).await?;

        self.session_ctx
            .set_query_context_shared(Arc::downgrade(&shared));
        Ok(QueryContext::create_from_shared(shared))
    }

    pub fn get_format_settings(&self) -> Result<FormatSettings> {
        let settings = &self.session_ctx.get_settings();
        let quote_char = settings.get_quote_char()?.into_bytes();
        if quote_char.len() != 1 {
            return Err(ErrorCode::InvalidArgument(
                "quote_char can only contain one char",
            ));
        }

        let mut format = FormatSettings {
            record_delimiter: settings.get_record_delimiter()?.into_bytes(),
            field_delimiter: settings.get_field_delimiter()?.into_bytes(),
            empty_as_default: settings.get_empty_as_default()? > 0,
            quote_char: quote_char[0],
            ..Default::default()
        };

        let tz = settings.get_timezone()?;
        format.timezone = tz.parse::<Tz>().map_err(|_| {
            ErrorCode::InvalidTimezone("Timezone has been checked and should be valid")
        })?;

        format.ident_case_sensitive = settings.get_unquoted_ident_case_sensitive()?;
        Ok(format)
    }

    pub fn get_current_query_id(&self) -> Option<String> {
        self.session_ctx.get_current_query_id()
    }

    pub fn attach<F>(self: &Arc<Self>, host: Option<SocketAddr>, io_shutdown: F)
    where F: FnOnce() + Send + 'static {
        let (tx, rx) = oneshot::channel();
        self.session_ctx.set_client_host(host);
        self.session_ctx.set_io_shutdown_tx(Some(tx));

        common_base::base::tokio::spawn(async move {
            if let Ok(tx) = rx.await {
                (io_shutdown)();
                tx.send(()).ok();
            }
        });
    }

    pub fn set_current_database(self: &Arc<Self>, database_name: String) {
        self.session_ctx.set_current_database(database_name);
    }

    pub fn get_current_database(self: &Arc<Self>) -> String {
        self.session_ctx.get_current_database()
    }

    pub fn get_current_catalog(self: &Arc<Self>) -> String {
        self.session_ctx.get_current_catalog()
    }

    pub fn get_current_tenant(self: &Arc<Self>) -> String {
        self.session_ctx.get_current_tenant()
    }

    pub fn set_current_tenant(self: &Arc<Self>, tenant: String) {
        self.session_ctx.set_current_tenant(tenant);
    }

    pub fn get_current_user(self: &Arc<Self>) -> Result<UserInfo> {
        self.session_ctx
            .get_current_user()
            .ok_or_else(|| ErrorCode::AuthenticateFailure("unauthenticated"))
    }

    pub fn set_current_user(self: &Arc<Self>, user: UserInfo) {
        self.session_ctx.set_current_user(user);
    }

    pub fn set_auth_role(self: &Arc<Self>, role: String) {
        self.session_ctx.set_auth_role(role)
    }

    // returns all the roles the current session has, which includes the roles of
    // the current user and the roles granted on the authentication phase.
    pub fn get_all_roles(self: &Arc<Self>) -> Result<Vec<String>> {
        let current_user = self.get_current_user()?;

        let mut all_roles = current_user.grants.roles();
        if let Some(auth_role) = self.session_ctx.get_auth_role() {
            all_roles.push(auth_role);
        }
        Ok(all_roles)
    }

    pub async fn validate_privilege(
        self: &Arc<Self>,
        object: &GrantObject,
        privilege: UserPrivilegeType,
    ) -> Result<()> {
        let current_user = self.get_current_user()?;
        let user_verified = current_user.grants.verify_privilege(object, privilege);
        if user_verified {
            return Ok(());
        }

        // TODO: take current role instead of all roles
        let all_roles = self.get_all_roles()?;
        let tenant = self.get_current_tenant();
        let role_verified = RoleCacheManager::instance()
            .find_related_roles(&tenant, &all_roles)
            .await?
            .iter()
            .any(|r| r.grants.verify_privilege(object, privilege));
        if role_verified {
            return Ok(());
        }

        Err(ErrorCode::PermissionDenied(format!(
            "Permission denied, user {} requires {} privilege on {}",
            &current_user.identity(),
            privilege,
            object
        )))
    }

    pub fn get_settings(self: &Arc<Self>) -> Arc<Settings> {
        self.session_ctx.get_settings()
    }

    pub fn get_changed_settings(self: &Arc<Self>) -> Arc<Settings> {
        self.session_ctx.get_changed_settings()
    }

    pub fn apply_changed_settings(self: &Arc<Self>, changed_settings: Arc<Settings>) -> Result<()> {
        self.session_ctx.apply_changed_settings(changed_settings)
    }

    pub fn get_memory_usage(self: &Arc<Self>) -> usize {
        // TODO(winter): use thread memory tracker
        0
    }

    pub fn get_config(&self) -> Config {
        SessionManager::instance().get_conf()
    }

    pub fn get_status(self: &Arc<Self>) -> Arc<RwLock<SessionStatus>> {
        self.status.clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        tracing::debug!("Drop session {}", self.id.clone());
        SessionManager::instance().destroy_session(&self.id.clone());
    }
}
