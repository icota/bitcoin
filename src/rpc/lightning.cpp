// Copyright (c) 2009-2019 The Bitcoin Core developers
// Distributed under the MIT software license, see the accompanying
// file COPYING or http://www.opensource.org/licenses/mit-license.php.

#include <rpc/server.h>

#include <rusty/src/rust_bridge.h>

#include <rpc/util.h>
#include <util/strencodings.h>

#include <univalue.h>

//! Pointer to Lightning node that needs to be declared as a global to be accessible
//! RPC methods. Due to limitations of the RPC framework, there's currently no
//! direct way to pass in state to RPC methods without globals.
extern void* lightning_node;

static UniValue lnconnect(const JSONRPCRequest& request)
{
    RPCHelpMan{"lnconnect",
               "\nConnect to a Lightning Network node.\n",
               {{"node", RPCArg::Type::STR, RPCArg::Optional::NO, "Node to connect to in pubkey@host:port format."},},
               RPCResult{
                       "n          (numeric) FIX\n"
               },
               RPCExamples{
                       HelpExampleCli("lnconnect", "pubkey@host:port")
                       + HelpExampleRpc("lnconnect", "pubkey@host:port")
               },
    }.Check(request);

//    if(!g_rpc_node->connman)
//        throw JSONRPCError(RPC_CLIENT_P2P_DISABLED, "Error: Peer-to-peer functionality missing or disabled");

    std::string strNode = request.params[0].get_str();

    return lightning::connect_peer(lightning_node, strNode.c_str());
    //return 0;
    //return (int)g_rpc_node->connman->GetNodeCount(CConnman::CONNECTIONS_ALL);
}

static UniValue lngetpeers(const JSONRPCRequest& request)
{
    RPCHelpMan{"lngetpeers",
               "\nList all connected Lightning Network nodes.\n",
               {},
               RPCResult{
                       "n          (numeric) FIXME\n"
               },
               RPCExamples{
                       HelpExampleCli("lngetpeers", "")
                       + HelpExampleRpc("lngetpeers", "")
               },
    }.Check(request);

//    if(!g_rpc_node->connman)
//        throw JSONRPCError(RPC_CLIENT_P2P_DISABLED, "Error: Peer-to-peer functionality missing or disabled");

    return lightning::get_peers(lightning_node);
}

// clang-format off
static const CRPCCommand commands[] =
        { //  category              name                      actor (function)         argNames
                //  --------------------- ------------------------  -----------------------  ----------
                { "lightning",            "lnconnect",                &lnconnect,                {"node"} },
                { "lightning",            "lngetpeers",               &lngetpeers,               {} },
        };
// clang-format on

void RegisterLightningRPCCommands(CRPCTable &t)
{
    for (unsigned int vcidx = 0; vcidx < ARRAYLEN(commands); vcidx++)
        t.appendCommand(commands[vcidx].name, &commands[vcidx]);
}