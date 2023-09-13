// Copyright 2022-2023 Protocol Labs
// SPDX-License-Identifier: MIT
//! RPC subnet cli command handler.

use async_trait::async_trait;
use clap::Args;
use ipc_sdk::subnet_id::SubnetID;
use std::fmt::Debug;
use std::str::FromStr;

use crate::{get_ipc_provider, CommandLineHandler, GlobalArguments};

/// The command to get the RPC endpoint for a subnet
pub struct RPCSubnet;

#[async_trait]
impl CommandLineHandler for RPCSubnet {
    type Arguments = RPCSubnetArgs;

    async fn handle(global: &GlobalArguments, arguments: &Self::Arguments) -> anyhow::Result<()> {
        log::debug!("get rpc for subnet with args: {:?}", arguments);

        let provider = get_ipc_provider(global)?;
        let subnet = SubnetID::from_str(&arguments.subnet)?;
        let conn = match provider.connection(&subnet) {
            None => return Err(anyhow::anyhow!("target subnet not found")),
            Some(conn) => conn,
        };

        println!("rpc: {:?}", conn.subnet().rpc_http().to_string());
        println!("chainID: {:?}", conn.manager().get_chain_id().await?);
        Ok(())
    }
}

#[derive(Debug, Args)]
#[command(name = "rpc", about = "RPC endpoint for a subnet")]
pub struct RPCSubnetArgs {
    #[arg(long, short, help = "The JSON RPC server url for ipc agent")]
    pub ipc_agent_url: Option<String>,
    #[arg(long, short, help = "The subnet to get the RPC from")]
    pub subnet: String,
}
