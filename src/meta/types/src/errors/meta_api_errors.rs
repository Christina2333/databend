// Copyright 2022 Datafuse Labs.
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

use std::fmt::Display;

use anyerror::AnyError;

use crate::raft_types::ChangeMembershipError;
use crate::raft_types::Fatal;
use crate::raft_types::ForwardToLeader;
use crate::ClientWriteError;
use crate::InvalidReply;
use crate::MetaNetworkError;
use crate::RaftError;

/// Errors raised when meta-service handling a request.
#[derive(thiserror::Error, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MetaAPIError {
    /// If a request can only be dealt with by a leader, it informs the caller to forward the request to a leader.
    #[error(transparent)]
    ForwardToLeader(#[from] ForwardToLeader),

    #[error("can not forward any more: {0}")]
    CanNotForward(AnyError),

    /// Network error when sending a request to the leader.
    #[error(transparent)]
    NetworkError(#[from] MetaNetworkError),

    /// Error occurs on local peer.
    #[error(transparent)]
    DataError(#[from] MetaDataError),

    /// Error occurs on remote peer.
    ///
    /// A server side API does not emit such an error.
    /// This is used for client-side to build a remote-error when receiving server errors
    #[error(transparent)]
    RemoteError(MetaDataError),
}

/// Errors raised when handling a request by raft node.
#[derive(thiserror::Error, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MetaOperationError {
    /// If a request can only be dealt by a leader, it informs the caller to forward the request to a leader it knows of.
    #[error(transparent)]
    ForwardToLeader(#[from] ForwardToLeader),

    #[error(transparent)]
    DataError(#[from] MetaDataError),
}

impl From<MetaOperationError> for MetaAPIError {
    fn from(e: MetaOperationError) -> Self {
        match e {
            MetaOperationError::ForwardToLeader(e) => e.into(),
            MetaOperationError::DataError(d) => d.into(),
        }
    }
}

/// Errors raised when read or write meta data locally.
#[derive(thiserror::Error, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum MetaDataError {
    /// Error occurred when writing a raft log.
    #[error(transparent)]
    WriteError(#[from] Fatal),

    /// Error occurred when change raft membership.
    #[error(transparent)]
    ChangeMembershipError(#[from] ChangeMembershipError),

    /// Error occurred when reading.
    #[error(transparent)]
    ReadError(#[from] MetaDataReadError),
}

/// Error occurred when a meta-node reads data.
#[derive(thiserror::Error, serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[error("fail to {action}: {msg}, source: {source}")]
pub struct MetaDataReadError {
    action: String,
    msg: String,
    #[source]
    source: AnyError,
}

impl MetaDataReadError {
    pub fn new(
        action: impl Display,
        msg: impl Display,
        source: &(impl std::error::Error + 'static),
    ) -> Self {
        Self {
            action: action.to_string(),
            msg: msg.to_string(),
            source: AnyError::new(source),
        }
    }
}

impl From<MetaDataReadError> for MetaOperationError {
    fn from(e: MetaDataReadError) -> Self {
        let de = MetaDataError::from(e);
        MetaOperationError::from(de)
    }
}

impl From<InvalidReply> for MetaAPIError {
    fn from(e: InvalidReply) -> Self {
        let net_err = MetaNetworkError::from(e);
        Self::NetworkError(net_err)
    }
}

impl From<RaftError<ClientWriteError>> for MetaAPIError {
    fn from(value: RaftError<ClientWriteError>) -> Self {
        match value {
            RaftError::APIError(cli_write_err) => {
                //
                match cli_write_err {
                    ClientWriteError::ForwardToLeader(f) => MetaAPIError::ForwardToLeader(f),
                    ClientWriteError::ChangeMembershipError(c) => {
                        MetaAPIError::DataError(MetaDataError::ChangeMembershipError(c))
                    }
                }
            }
            RaftError::Fatal(f) => MetaAPIError::DataError(MetaDataError::WriteError(f)),
        }
    }
}
