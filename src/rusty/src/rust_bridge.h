// Rust functions which are exposed to C++ (ie are #[no_mangle] pub extern "C")
// Copyright (c) 2009-2019 The Bitcoin Core developers
// Distributed under the MIT software license, see the accompanying
// file COPYING or http://www.opensource.org/licenses/mit-license.php.

#ifndef BITCOIN_RUSTY_SRC_RUST_BRIDGE_H
#define BITCOIN_RUSTY_SRC_RUST_BRIDGE_H

#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <new>

namespace rust_block_fetch {

extern "C" {

bool init_fetch_dns_headers(const char *domain);
bool stop_fetch_dns_headers();
bool init_fetch_rest_blocks(const char *uri);
bool stop_fetch_rest_blocks();
bool init_p2p_client(void* pconnman, const char *datadir, const char *subver, uint16_t bind_port, const char **dnsseed_names, size_t dnsseed_count);
bool stop_p2p_client();

static const bool DEFAULT_P2P = true;
} // extern "C"

} // namespace rust_block_fetch

namespace lightning {

extern "C" {

static const bool DEFAULT_LIGHTNING = true;
static const int DEFAULT_LIGHTNING_PORT = 9735;

void* init_node(void* pconnman, const char *datadir, const char *subver);
bool listen_incoming(void* node_ptr, uint16_t bind_port);
bool connect_peer(void* node_ptr, const char *addr);
bool get_peers(void* node_ptr);

} // extern "C"

} // namespace lightning


#endif // BITCOIN_RUSTY_SRC_RUST_BRIDGE_H
