// Copyright 2021 Datafuse Labs
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

use std::{path::Path, vec};

use chrono::{NaiveDateTime, Utc};
use databend_driver::{Client, LoadMethod};
use tokio_stream::StreamExt;

use crate::common::DEFAULT_DSN;

async fn prepare_client(presigned: bool) -> Option<Client> {
    let dsn = option_env!("TEST_DATABEND_DSN").unwrap_or(DEFAULT_DSN);
    let client = if presigned {
        Client::new(format!("{dsn}&presign=on"))
    } else {
        Client::new(format!("{dsn}&presign=off"))
    };
    let conn = client.get_conn().await.unwrap();
    let info = conn.info().await;
    if info.handler == "FlightSQL" {
        // NOTE: FlightSQL does not support stream load
        return None;
    }
    Some(client)
}

async fn prepare_table(client: &Client, prefix: &str, load_method: Option<LoadMethod>) -> String {
    let method = if let Some(load_method) = load_method {
        format!("{load_method:?}").to_lowercase()
    } else {
        "attachment".to_string()
    };
    let conn = client.get_conn().await.unwrap();
    let table = format!(
        "books_{prefix}_{}_{method}",
        Utc::now().format("%Y%m%d%H%M%S%9f")
    );
    let sql = format!(
        "CREATE TABLE `{table}` (
            title VARCHAR NULL,
            author VARCHAR NULL,
            date VARCHAR NULL,
            publish_time TIMESTAMP NULL)",
    );
    conn.exec(&sql, ()).await.unwrap();
    table
}

async fn prepare_data(table: &str, client: &Client, load_method: LoadMethod) {
    let conn = client.get_conn().await.unwrap();
    let sql = format!("INSERT INTO `{table}` VALUES");
    let data = vec![
        vec![
            "Transaction Processing",
            "Jim Gray",
            "1992",
            "2020-01-01 11:11:11.345",
        ],
        vec![
            "Readings in Database Systems",
            "Michael Stonebraker",
            "2004",
            "2020-01-01T11:11:11Z",
        ],
        vec!["Three Body", "NULL-liucixin", "2019", "2019-07-04T00:00:00"],
    ];
    let stats = conn.stream_load(&sql, data, load_method).await.unwrap();
    assert_eq!(stats.write_rows, 3);
}

async fn prepare_data_with_file(
    table: &str,
    file_type: &str,
    client: &Client,
    load_method: Option<LoadMethod>,
) {
    let conn = client.get_conn().await.unwrap();
    let fp = format!("tests/driver/data/books.{file_type}");
    let stats = if let Some(load_method) = load_method {
        let sql =
            format!("INSERT INTO `{table}` VALUES from @_databend_load file_format=(type=csv)");
        conn.load_file(&sql, Path::new(&fp), load_method)
            .await
            .unwrap()
    } else {
        let sql = format!("INSERT INTO `{table}` VALUES");
        conn.load_file_with_options(&sql, Path::new(&fp), None, None)
            .await
            .unwrap()
    };
    assert_eq!(stats.write_rows, 3);
}

async fn check_result(table: &str, client: &Client) {
    let conn = client.get_conn().await.unwrap();
    let sql = format!("SELECT * FROM `{table}`");
    let rows = conn.query_iter(&sql, ()).await.unwrap();
    let result: Vec<(String, String, String, NaiveDateTime)> =
        rows.map(|r| r.unwrap().try_into().unwrap()).collect().await;

    let expected = vec![
        (
            "Transaction Processing".to_string(),
            "Jim Gray".to_string(),
            "1992".to_string(),
            NaiveDateTime::parse_from_str("2020-01-01 11:11:11.345", "%Y-%m-%d %H:%M:%S%.f")
                .unwrap(),
        ),
        (
            "Readings in Database Systems".to_string(),
            "Michael Stonebraker".to_string(),
            "2004".to_string(),
            NaiveDateTime::parse_from_str("2020-01-01T11:11:11Z", "%Y-%m-%dT%H:%M:%SZ").unwrap(),
        ),
        (
            "Three Body".to_string(),
            "NULL-liucixin".to_string(),
            "2019".to_string(),
            NaiveDateTime::parse_from_str("2019-07-04T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap(),
        ),
    ];
    assert_eq!(result, expected);

    let sql = format!("DROP TABLE `{table}`;");
    conn.exec(&sql, ()).await.unwrap();
}

#[tokio::test]
async fn load_csv_with_presign() {
    if let Some(client) = prepare_client(true).await {
        for m in [None, Some(LoadMethod::Streaming), Some(LoadMethod::Stage)] {
            let table = prepare_table(&client, "load_csv_with_presign", m).await;
            prepare_data_with_file(&table, "csv", &client, m).await;
            check_result(&table, &client).await;
        }
    }
}

#[tokio::test]
async fn load_csv_without_presign() {
    if let Some(client) = prepare_client(false).await {
        for m in [Some(LoadMethod::Streaming), Some(LoadMethod::Stage)] {
            let table = prepare_table(&client, "load_csv_without_presign", m).await;
            prepare_data_with_file(&table, "csv", &client, m).await;
            check_result(&table, &client).await;
        }
    }
}

#[tokio::test]
async fn stream_load_with_presign() {
    if let Some(client) = prepare_client(true).await {
        for m in [LoadMethod::Streaming, LoadMethod::Stage] {
            let table = prepare_table(&client, "stream_load_with_presign", Some(m)).await;
            prepare_data(&table, &client, m).await;
            check_result(&table, &client).await;
        }
    }
}

#[tokio::test]
async fn stream_load_without_presign() {
    if let Some(client) = prepare_client(false).await {
        for m in [LoadMethod::Streaming, LoadMethod::Stage] {
            let table = prepare_table(&client, "stream_load_without_presign", Some(m)).await;
            prepare_data(&table, &client, m).await;
            check_result(&table, &client).await;
        }
    }
}
