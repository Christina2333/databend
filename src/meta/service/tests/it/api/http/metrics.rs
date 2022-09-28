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

use common_base::base::tokio;
use common_metrics::init_default_metrics_recorder;
use databend_meta::api::http::v1::metrics::metrics_handler;
use maplit::btreeset;
use poem::get;
use poem::http::Method;
use poem::http::StatusCode;
use poem::http::Uri;
use poem::Endpoint;
use poem::EndpointExt;
use poem::Request;
use poem::Route;
use pretty_assertions::assert_eq;
use tracing::info;

use crate::init_meta_ut;
use crate::tests::meta_node::start_meta_node_cluster;

#[async_entry::test(worker_threads = 3, init = "init_meta_ut!()", tracing_span = "debug")]
async fn test_metrics() -> anyhow::Result<()> {
    init_default_metrics_recorder();

    let (_, tcs) = start_meta_node_cluster(btreeset! {0,1,2}, btreeset! {}).await?;

    let leader = tcs[0].meta_node.clone().unwrap();

    let cluster_router = Route::new()
        .at("/v1/metrics", get(metrics_handler))
        .data(leader);

    let mut response = cluster_router
        .call(
            Request::builder()
                .uri(Uri::from_static("/v1/metrics"))
                .method(Method::GET)
                .finish(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Sample output:
    // metasrv_server_leader_changes 3
    // metasrv_raft_network_recv_bytes{from="127.0.0.1:62268"} 1752
    // metasrv_raft_network_recv_bytes{from="127.0.0.1:62270"} 1535
    // metasrv_raft_network_sent_bytes{to="1"} 1752
    // metasrv_raft_network_sent_bytes{to="2"} 1535
    // metasrv_server_last_log_index 6
    // metasrv_server_proposals_pending 0
    // metasrv_server_is_leader 0
    // metasrv_server_node_is_health 1
    // metasrv_server_last_seq 0
    // metasrv_raft_network_active_peers{id="2",address="127.0.0.1:29006"} 0
    // metasrv_raft_network_active_peers{id="1",address="127.0.0.1:29003"} 1
    // metasrv_server_proposals_applied 6
    // metasrv_server_current_leader_id 0
    // metasrv_server_current_term 1

    let b = response.take_body();
    let txt = b.into_string().await?;
    info!("metrics response text: {}", txt);

    let metric_keys = {
        let lines = txt.split('\n');
        let mut metric_keys = btreeset! {};

        for line in lines {
            if line.starts_with('#') {
                continue;
            }
            if line.is_empty() {
                continue;
            }

            let mut segments = line.split(' ');
            let key = segments.next().unwrap();
            metric_keys.insert(key);
            info!("found response metric key: {:?}", key);
        }
        metric_keys
    };

    // Only static keys are checked.

    assert!(metric_keys.contains("metasrv_server_leader_changes"));
    assert!(metric_keys.contains("metasrv_server_last_log_index"));
    assert!(metric_keys.contains("metasrv_server_proposals_pending"));
    assert!(metric_keys.contains("metasrv_server_is_leader"));
    assert!(metric_keys.contains("metasrv_server_node_is_health"));
    assert!(metric_keys.contains("metasrv_server_last_seq"));
    assert!(metric_keys.contains("metasrv_server_proposals_applied"));
    assert!(metric_keys.contains("metasrv_server_current_leader_id"));
    assert!(metric_keys.contains("metasrv_server_current_term"));

    Ok(())
}
