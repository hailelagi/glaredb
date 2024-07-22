#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

use rayexec_error::Result;
use rayexec_parser::statement::Statement;
use serde::{de::DeserializeSeed, Deserializer, Serialize};

use crate::{
    database::DatabaseContext,
    datasource::DataSourceRegistry,
    execution::{pipeline::PartitionPipeline, query_graph::QueryGraph},
    logical::sql::binder::{BindData, BoundStatement},
    runtime::{ExecutionRuntime, QueryHandle},
};
use std::sync::Arc;

/// A "server" session for doing remote planning and remote execution.
///
/// Keeps no state and very cheap to create. Essentially just encapsulates logic
/// for what should happen on the remote side for hybrid/distributed execution.
#[derive(Debug)]
pub struct ServerSession {
    /// Context this session has access to.
    context: DatabaseContext,

    /// Registered data source implementations.
    registry: Arc<DataSourceRegistry>,

    /// Query runtime.
    runtime: Arc<dyn ExecutionRuntime>,
}

impl ServerSession {
    pub fn new(
        context: DatabaseContext,
        runtime: Arc<dyn ExecutionRuntime>,
        registry: Arc<DataSourceRegistry>,
    ) -> Self {
        ServerSession {
            context,
            registry,
            runtime,
        }
    }

    /// Completes binding for a statement.
    ///
    /// Failing to complete binding (e.g. unable to resolve a table) should
    /// result in an error. Otherwise we can assume that all references are
    /// bound and we can continue with planning for hybrid exec.
    pub async fn complete_binding(
        &self,
        stmt: BoundStatement,
        bind_data: BindData,
    ) -> Result<(BoundStatement, BindData)> {
        // TODO: Check the statement and complete anything pending.
        // Straightforward.
        unimplemented!()
    }

    /// Plans a hyrbid query graph from a completely bound statement.
    pub fn plan_hybrid_graph(
        &self,
        stmt: BoundStatement,
        bind_data: BindData,
    ) -> Result<QueryGraph> {
        // TODO: Statement -> logical with typical planning.
        //
        // Logical -> "stateless" pipeline. Will not be returning a query graph,
        // but pre-marked pipelines with locations where to execute.
        //
        // Handler should serialize "client" pipelines and send back to client.
        // "server" pipelines should immediately start executing.
        unimplemented!()
    }

    pub fn execute_pipelines(&self, pipelines: Vec<PartitionPipeline>) {
        // TODO: Accept "stateless" pipelines. Inflate with states. Execute.
        //
        // Return something to allow remote cancelation (uuid).
        //
        // Probably change ExecutionRuntime to handle "state inflation" on
        // "stateless" pipelines.
        unimplemented!()
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct HybridExecuteRequest {
    pub stmt: BoundStatement,
    pub bind_data: BindData,
}

#[derive(Debug)]
struct HybridExecuteRequestDeserializer<'a> {
    context: &'a DatabaseContext,
}

impl<'de, 'a> DeserializeSeed<'de> for HybridExecuteRequestDeserializer<'a> {
    type Value = HybridExecuteRequest;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        unimplemented!()
    }
}