use bitcoin::secp256k1::key::PublicKey;
use bitcoin::secp256k1::Secp256k1;

use std::os::raw::c_char;
use std::ffi::{c_void, CStr};
use bitcoin::network::constants;
use std::sync::{Arc, Mutex};

use crate::bridge::*;
use std::{fs, cmp};
use std::io::{Write, Cursor};
use lightning::chain::keysinterface::{KeysManager, KeysInterface, SpendableOutputDescriptor, ChannelKeys, InMemoryChannelKeys};
use lightning::chain::chaininterface;
use lightning::chain::chaininterface::{ChainError, ChainWatchInterface};
use bitcoin::util::bip32;
use bitcoin_hashes::hex::{ToHex, FromHex};
use bitcoin::consensus::serialize;
use lightning::util::logger::{Logger, Record};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use std::collections::HashMap;
use bitcoin::{Network, blockdata, Block, Transaction};

use std::sync::atomic::{AtomicUsize, Ordering};
use lightning::ln::{channelmonitor, channelmanager, router, peer_handler};
use lightning::chain;
use lightning::ln::channelmonitor::ManyChannelMonitor;
use lightning::util::ser::{ReadableArgs, Writeable};
use lightning::util::config;
use lightning::ln::peer_handler::PeerManager;

use crate::lightning_socket_handler;
use lightning::ln::channelmanager::{PaymentHash, PaymentPreimage};
use lightning_socket_handler::Connection;
use std::net::{TcpListener, SocketAddr, SocketAddrV4, Ipv4Addr};
use std::mem::transmute;
use lightning::util::events::{EventsProvider, Event};


const FEE_PROPORTIONAL_MILLIONTHS: u32 = 10;
const ANNOUNCE_CHANNELS: bool = false;
static THREAD_COUNT: AtomicUsize = AtomicUsize::new(0);

#[no_mangle]
pub extern fn init_node(connman_ptr: *mut c_void, datadir_path: *const c_char, subver_c: *const c_char) -> *mut Node {
    let lightning_ptr = unsafe { transmute(Box::new(Node::new(connman_ptr, datadir_path, subver_c))) };
    lightning_ptr
}

#[no_mangle]
pub extern fn listen_incoming(ptr: *mut Node, bind_port: u16) -> bool {
    let node = unsafe { &mut *ptr };
    node.listen_incoming(bind_port)
}

#[no_mangle]
pub extern fn connect_peer(ptr: *mut Node, addr: *const c_char) -> bool {
    let node = unsafe { &mut *ptr };
    node.connect_peer(addr)
}

#[no_mangle]
pub extern fn get_peers(ptr: *mut Node) -> bool {
    let node = unsafe { &mut *ptr };
    node.get_peers()
}

pub struct Node {
    peer_manager: Arc<PeerManager<lightning_socket_handler::SocketDescriptor>>,
    event_handler: Arc<EventHandler>, // TODO: is there an event handler in peer_manager?
}

impl Node {
    pub fn new(connman_ptr: *mut c_void, datadir_path: *const c_char, subver_c: *const c_char) -> Node {

//    #[no_mangle]
//    pub extern "C" fn init_lightning(connman_ptr: *mut c_void, datadir_path: *const c_char, subver_c: *const c_char, bind_port: u16) {
        // what does connman do in p2p? we probably don't need it
        let connman = Connman(connman_ptr);

        let data_path: String = match unsafe { CStr::from_ptr(datadir_path) }.to_str() {
            Ok(d) => d.to_string(), // + "/lightning.dat",
            Err(_) => panic!(),
        };

        let subver: String = match unsafe { CStr::from_ptr(subver_c) }.to_str() {
            Ok(d) => d.to_string(),
            Err(_) => panic!(),
        };

        print!("Version: {}\n", subver);
        let mut network = constants::Network::Bitcoin;

        print!("Network: {}\n", BlockIndex::network_id());

        match BlockIndex::network_id().as_ref() {
            "main" => network = constants::Network::Bitcoin,
            "test" => network = constants::Network::Testnet,
            "regtest" => network = constants::Network::Regtest,
            _ => panic!("Unknown network"),
        }

        let our_node_seed = if let Ok(seed) = fs::read(data_path.clone() + "/key_seed") {
            assert_eq!(seed.len(), 32);
            let mut key = [0; 32];
            key.copy_from_slice(&seed);
            key
        } else {
            let mut rand_ctx = RandomContext::new();
            let seed = rand_ctx.randthirtytwobytes();
            let mut key = [0; 32];
            key.copy_from_slice(serialize(&seed).as_slice());
            let mut f = fs::File::create(data_path.clone() + "/key_seed").unwrap();
            f.write_all(&key).expect("Failed to write seed to disk");
            f.sync_all().expect("Failed to sync seed to disk");
            key
        };


        let logger = Arc::new(LogPrinter {});

        let starting_time = SystemTime::now();
        let since_the_epoch = starting_time.duration_since(UNIX_EPOCH)
            .expect("Time went backwards");

        let keys = Arc::new(KeysManager::new(&our_node_seed, network, logger.clone(), since_the_epoch.as_secs(), since_the_epoch.subsec_micros()));

        let secp_ctx = Secp256k1::new();

        // ignore import keys for now

        let (import_key_1, import_key_2) = bip32::ExtendedPrivKey::new_master(network, &our_node_seed).map(|extpriv| {
            (extpriv.ckd_priv(&secp_ctx, bip32::ChildNumber::from_hardened_idx(1).unwrap()).unwrap().private_key.key,
             extpriv.ckd_priv(&secp_ctx, bip32::ChildNumber::from_hardened_idx(2).unwrap()).unwrap().private_key.key)
        }).unwrap();

        let chain_monitor = Arc::new(ChainInterface::new(network, logger.clone()));

        // rejig this guy to use ffi
        let fee_estimator = Arc::new(FeeEstimator::new());

        let _ = fs::create_dir(data_path.clone() + "/monitors");


        // hugeass tokio runtime block here ---> first thing it imports keys to bitcoin

        // why does it import two keys?


        // DO THAT OR FAIL
        // NEXT

        // no such file ---> fix it
        let mut monitors_loaded = ChannelMonitor::load_from_disk(&(data_path.clone() + "/monitors"));
        let monitor = Arc::new(ChannelMonitor {
            monitor: channelmonitor::SimpleManyChannelMonitor::new(chain_monitor.clone(), chain_monitor.clone(), logger.clone(), fee_estimator.clone()),
            file_prefix: data_path.clone() + "/monitors",
        });

        let mut config = config::UserConfig::default();
        config.channel_options.fee_proportional_millionths = FEE_PROPORTIONAL_MILLIONTHS;
        config.channel_options.announced_channel = ANNOUNCE_CHANNELS;

        let channel_manager = if let Ok(mut f) = fs::File::open(data_path.clone() + "/manager_data") {
            let (last_block_hash, manager) = {
                let mut monitors_refs = HashMap::new();
                for (outpoint, monitor) in monitors_loaded.iter_mut() {
                    monitors_refs.insert(*outpoint, monitor);
                }
                <(bitcoin_hashes::sha256d::Hash, channelmanager::ChannelManager<InMemoryChannelKeys>)>::read(&mut f, channelmanager::ChannelManagerReadArgs {
                    keys_manager: keys.clone(),
                    fee_estimator: fee_estimator.clone(),
                    monitor: monitor.clone(),
                    tx_broadcaster: chain_monitor.clone(),
                    logger: logger.clone(),
                    default_config: config,
                    channel_monitors: &mut monitors_refs,
                }).expect("Failed to deserialize channel manager")
            };
            monitor.load_from_vec(monitors_loaded);
            //TODO: Rescan
            let manager = Arc::new(manager);
            let manager_as_listener: Arc<chaininterface::ChainListener> = manager.clone();
            //chain_monitor.register_listener(Arc::downgrade(&manager_as_listener));
            manager
        } else {
            if !monitors_loaded.is_empty() {
                panic!("Found some channel monitors but no channel state!");
            }
            channelmanager::ChannelManager::new(network, fee_estimator.clone(), monitor.clone(), chain_monitor.clone(), logger.clone(), keys.clone(), config, 500000).unwrap() //todo: give it the height here
        };

        let router = Arc::new(router::Router::new(PublicKey::from_secret_key(&secp_ctx, &keys.get_node_secret()), chain_monitor.clone(), logger.clone()));

        let mut rand_ctx = RandomContext::new();
        let rand256 = rand_ctx.randthirtytwobytes();
        let mut rand = [0; 32];
        rand.copy_from_slice(serialize(&rand256).as_slice());

        // TODO: implement socketdescriptor without using tokio ---> look at p2p_socket_handler

        let peer_manager : Arc<PeerManager<lightning_socket_handler::SocketDescriptor>> = Arc::new(peer_handler::PeerManager::new(peer_handler::MessageHandler {
            chan_handler: channel_manager.clone(),
            route_handler: router.clone(),
        }, keys.get_node_secret(), &rand,logger.clone()));

        let payment_preimages : Arc<Mutex<HashMap<PaymentHash, PaymentPreimage>>> = Arc::new(Mutex::new(HashMap::new()));

        let mut event_handler = Arc::new(EventHandler::new(network, data_path, peer_manager.clone(), monitor.monitor.clone(), channel_manager.clone(), chain_monitor.clone(), payment_preimages.clone()));


        Node {
            peer_manager,
            event_handler
        }

//    tokio::spawn(listener.incoming().for_each(move |sock| {
//        println!("Got new inbound connection, waiting on them to start handshake...");
//        Connection::setup_inbound(peer_manager_listener.clone(), event_listener.clone(), sock);
//        Ok(())
//    }).then(|_| { Ok(()) }));
//
//    spawn_chain_monitor(fee_estimator, rpc_client, chain_monitor, event_notify.clone());
//
//    tokio::spawn(tokio::timer::Interval::new(Instant::now(), Duration::new(1, 0)).for_each(move |_| {
//        //TODO: Regularly poll chain_monitor.txn_to_broadcast and send them out
//        Ok(())
//    }).then(|_| { Ok(()) }));
    }

    pub fn listen_incoming(&self, bind_port: u16) -> bool {
        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0,0,0,0), bind_port))).unwrap();
        //listener.set_nonblocking(true).expect("Cannot set non-blocking");

        print!("Listening on port {}...\n", bind_port);

        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    Connection::setup_inbound(self.peer_manager.clone(), self.event_handler.clone(), s);
                }
                Err(e) => panic!("encountered IO error: {}", e),
            }
        };

        true
    }

    pub fn connect_peer(&self, addr: *const c_char) -> bool {
        let line: String = match unsafe { CStr::from_ptr(addr) }.to_str() {
            Ok(d) => d.to_string(),
            Err(_) => return false,
        };

        match hex_to_compressed_pubkey(&line) {
            Some(pk) => {
                if line.as_bytes()[33*2] == '@' as u8 {
                    let parse_res: Result<std::net::SocketAddr, _> = line.split_at(33*2 + 1).1.parse();
                    if let Ok(addr) = parse_res {
                        print!("Attempting to connect to {}...", addr);
                        match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
                            Ok(stream) => {
                                stream.set_nonblocking(false).expect("set_nonblocking call failed");
                                println!("connected, initiating handshake!");
                                Connection::setup_outbound(self.peer_manager.clone(), self.event_handler.clone(), pk, stream); //undo here
                                println!("connection setup!");
                            },
                            Err(e) => {
                                println!("connection failed {:?}!", e);
                            }
                        }
                    } else { println!("Couldn't parse host:port into a socket address"); }
                } else { println!("Invalid line, should be c pubkey@host:port"); }
            },
            None => println!("Bad PubKey for remote node"),
        };
        true
    }

    pub fn get_peers(&self) -> bool {
        let mut nodes = String::new();
        for node_id in self.peer_manager.get_peer_node_ids() {
            nodes += &format!("{}, ", hex_str(&node_id.serialize()));
        }
        println!("Connected nodes: {}", nodes);
        false
    }
}

pub fn hex_to_vec(hex: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(hex.len() / 2);

    let mut b = 0;
    for (idx, c) in hex.as_bytes().iter().enumerate() {
        b <<= 4;
        match *c {
            b'A'...b'F' => b |= c - b'A' + 10,
            b'a'...b'f' => b |= c - b'a' + 10,
            b'0'...b'9' => b |= c - b'0',
            _ => return None,
        }
        if (idx & 1) == 1 {
            out.push(b);
            b = 0;
        }
    }

    Some(out)
}

pub fn hex_to_compressed_pubkey(hex: &str) -> Option<PublicKey> {
    let data = match hex_to_vec(&hex[0..33*2]) {
        Some(bytes) => bytes,
        None => return None
    };
    match PublicKey::from_slice(&data) {
        Ok(pk) => Some(pk),
        Err(_) => None,
    }
}

#[inline]
pub fn hex_str(value: &[u8]) -> String {
    let mut res = String::with_capacity(64);
    for v in value {
        res += &format!("{:02x}", v);
    }
    res
}



struct LogPrinter {}
impl Logger for LogPrinter {
    fn log(&self, record: &Record) {
        if !record.args.to_string().contains("Received message of type 258") && !record.args.to_string().contains("Received message of type 256") && !record.args.to_string().contains("Received message of type 257") {
            // println for now --> fw to core logger in future
            println!("{:<5} [{} : {}, {}] {}", record.level.to_string(), record.module_path, record.file, record.line, record.args);
        }
    }
}

pub struct EventHandler {
    network: constants::Network,
    file_prefix: String,
    peer_manager: Arc<peer_handler::PeerManager<lightning_socket_handler::SocketDescriptor>>,
    channel_manager: Arc<channelmanager::ChannelManager<InMemoryChannelKeys>>,
    monitor: Arc<channelmonitor::SimpleManyChannelMonitor<chain::transaction::OutPoint>>,
    broadcaster: Arc<chain::chaininterface::BroadcasterInterface>,
    txn_to_broadcast: Mutex<HashMap<chain::transaction::OutPoint, blockdata::transaction::Transaction>>,
    payment_preimages: Arc<Mutex<HashMap<PaymentHash, PaymentPreimage>>>,
}
impl EventHandler {
    fn new(network: constants::Network, file_prefix: String, peer_manager: Arc<peer_handler::PeerManager<lightning_socket_handler::SocketDescriptor>>, monitor: Arc<channelmonitor::SimpleManyChannelMonitor<chain::transaction::OutPoint>>, channel_manager: Arc<channelmanager::ChannelManager<InMemoryChannelKeys>>, broadcaster: Arc<chain::chaininterface::BroadcasterInterface>, payment_preimages: Arc<Mutex<HashMap<PaymentHash, PaymentPreimage>>>) -> EventHandler {
        Self {
            network,
            file_prefix,
            peer_manager,
            channel_manager,
            monitor,
            broadcaster,
            txn_to_broadcast: Mutex::new(HashMap::new()),
            payment_preimages
        }
    }

    pub fn process_events(&self) {
        self.peer_manager.process_events();
        let mut events = self.channel_manager.get_and_clear_pending_events();
        events.append(&mut self.monitor.get_and_clear_pending_events()); // why append here? should be called twice
        for event in events {
            match event {
                Event::FundingGenerationReady { temporary_channel_id, channel_value_satoshis, output_script, .. } => {
                    let addr = bitcoin_bech32::WitnessProgram::from_scriptpubkey(&output_script[..], match self.network {
                        constants::Network::Bitcoin => bitcoin_bech32::constants::Network::Bitcoin,
                        constants::Network::Testnet => bitcoin_bech32::constants::Network::Testnet,
                        constants::Network::Regtest => bitcoin_bech32::constants::Network::Regtest,
                    }
                    ).expect("LN funding tx should always be to a SegWit output").to_address();
                    let us = self.clone();
//                        return future::Either::A(us.rpc_client.make_rpc_call("createrawtransaction", &["[]", &format!("{{\"{}\": {}}}", addr, channel_value_satoshis as f64 / 1_000_000_00.0)], false).and_then(move |tx_hex| {
//                            us.rpc_client.make_rpc_call("fundrawtransaction", &[&format!("\"{}\"", tx_hex.as_str().unwrap())], false).and_then(move |funded_tx| {
//                                let changepos = funded_tx["changepos"].as_i64().unwrap();
//                                assert!(changepos == 0 || changepos == 1);
//                                us.rpc_client.make_rpc_call("signrawtransactionwithwallet", &[&format!("\"{}\"", funded_tx["hex"].as_str().unwrap())], false).and_then(move |signed_tx| {
//                                    assert_eq!(signed_tx["complete"].as_bool().unwrap(), true);
//                                    let tx: blockdata::transaction::Transaction = encode::deserialize(&hex_to_vec(&signed_tx["hex"].as_str().unwrap()).unwrap()).unwrap();
//                                    let outpoint = chain::transaction::OutPoint {
//                                        txid: tx.txid(),
//                                        index: if changepos == 0 { 1 } else { 0 },
//                                    };
//                                    us.channel_manager.funding_transaction_generated(&temporary_channel_id, outpoint);
//                                    us.txn_to_broadcast.lock().unwrap().insert(outpoint, tx);
//                                    let _ = self_sender.try_send(());
//                                    println!("Generated funding tx!");
//                                    Ok(())
//                                })
//                            })
//                        }));
                },
                Event::FundingBroadcastSafe { funding_txo, .. } => {
                    let mut txn = self.txn_to_broadcast.lock().unwrap();
                    let tx = txn.remove(&funding_txo).unwrap();
                    self.broadcaster.broadcast_transaction(&tx);
                    println!("Broadcast funding tx {}!", tx.txid());
                },
                Event::PaymentReceived { payment_hash, amt } => {
                    let images = self.payment_preimages.lock().unwrap();
                    if let Some(payment_preimage) = images.get(&payment_hash) {
                        if self.channel_manager.claim_funds(payment_preimage.clone(), 10000) {
                            println!("Moneymoney! {} id {}", amt, hex_str(&payment_hash.0));
                        } else {
                            println!("Failed to claim money we were told we had?");
                        }
                    } else {
                        self.channel_manager.fail_htlc_backwards(&payment_hash);
                        println!("Received payment but we didn't know the preimage :(");
                    }
                    //let _ = self_sender.try_send(());
                },
                Event::PaymentSent { payment_preimage } => {
                    println!("Less money :(, proof: {}", hex_str(&payment_preimage.0));
                },
                Event::PaymentFailed { payment_hash, rejected_by_dest } => {
                    println!("{} failed id {}!", if rejected_by_dest { "Send" } else { "Route" }, hex_str(&payment_hash.0));
                },
                Event::PendingHTLCsForwardable { time_forwardable } => {
                    let us = self.clone();
//                        let mut self_sender = self_sender.clone();
//                        tokio::spawn(tokio::timer::Delay::new(time_forwardable).then(move |_| {
//                            us.channel_manager.process_pending_htlc_forwards();
//                            let _ = self_sender.try_send(());
//                            Ok(())
//                        }));
                },
                Event::SpendableOutputs { mut outputs } => {
                    for output in outputs.drain(..) {
                        match output {
                            SpendableOutputDescriptor::StaticOutput { outpoint, .. } => {
                                println!("Got on-chain output Bitcoin Core should know how to claim at {}:{}", hex_str(&outpoint.txid[..]), outpoint.vout);
                            },
                            SpendableOutputDescriptor::DynamicOutputP2WSH { .. } => {
                                println!("Got on-chain output we should claim...");
                                //TODO: Send back to Bitcoin Core!
                            },
                            SpendableOutputDescriptor::DynamicOutputP2WPKH { .. } => {
                                println!("Got on-chain output we should claim...");
                                //TODO: Send back to Bitcoin Core!
                            },
                        }
                    }
                },
            }
        }

        let filename = format!("{}/manager_data", self.file_prefix);
        let tmp_filename = filename.clone() + ".tmp";

        {
            let mut f = fs::File::create(&tmp_filename).unwrap();
            self.channel_manager.write(&mut f).unwrap();
        }

        fs::rename(&tmp_filename, &filename).unwrap();
    }
}

pub struct FeeEstimator {
    background_est: AtomicUsize,
    normal_est: AtomicUsize,
    high_prio_est: AtomicUsize,
}

impl FeeEstimator {
    pub fn new() -> Self {
        FeeEstimator {
            background_est: AtomicUsize::new(0),
            normal_est: AtomicUsize::new(0),
            high_prio_est: AtomicUsize::new(0),
        }
    }
    fn update_values(us: Arc<Self>) {
        //get a vector of 3 estimates with FFI like a man
//        let mut reqs: Vec<Box<Future<Item=(), Error=()> + Send>> = Vec::with_capacity(3);
//        {
//            let us = us.clone();
//            reqs.push(Box::new(rpc_client.make_rpc_call("estimatesmartfee", &vec!["6", "\"CONSERVATIVE\""], false).and_then(move |v| {
//                if let Some(serde_json::Value::Number(hp_btc_per_kb)) = v.get("feerate") {
//                    us.high_prio_est.store((hp_btc_per_kb.as_f64().unwrap() * 100_000_000.0 / 250.0) as usize + 3, Ordering::Release);
//                }
//                Ok(())
//            })));
//        }
//        {
//            let us = us.clone();
//            reqs.push(Box::new(rpc_client.make_rpc_call("estimatesmartfee", &vec!["18", "\"ECONOMICAL\""], false).and_then(move |v| {
//                if let Some(serde_json::Value::Number(np_btc_per_kb)) = v.get("feerate") {
//                    us.normal_est.store((np_btc_per_kb.as_f64().unwrap() * 100_000_000.0 / 250.0) as usize + 3, Ordering::Release);
//                }
//                Ok(())
//            })));
//        }
//        {
//            let us = us.clone();
//            reqs.push(Box::new(rpc_client.make_rpc_call("estimatesmartfee", &vec!["144", "\"ECONOMICAL\""], false).and_then(move |v| {
//                if let Some(serde_json::Value::Number(bp_btc_per_kb)) = v.get("feerate") {
//                    us.background_est.store((bp_btc_per_kb.as_f64().unwrap() * 100_000_000.0 / 250.0) as usize + 3, Ordering::Release);
//                }
//                Ok(())
//            })));
//        }
//        future::join_all(reqs).then(|_| { Ok(()) })
    }
}

impl chaininterface::FeeEstimator for FeeEstimator {
    fn get_est_sat_per_1000_weight(&self, conf_target: chaininterface::ConfirmationTarget) -> u64 {
        cmp::max(match conf_target {
            chaininterface::ConfirmationTarget::Background => self.background_est.load(Ordering::Acquire) as u64,
            chaininterface::ConfirmationTarget::Normal => self.normal_est.load(Ordering::Acquire) as u64,
            chaininterface::ConfirmationTarget::HighPriority => self.high_prio_est.load(Ordering::Acquire) as u64,
        }, 253)
    }
}

pub struct ChainInterface {
    util: chaininterface::ChainWatchInterfaceUtil,
    txn_to_broadcast: Mutex<HashMap<bitcoin_hashes::sha256d::Hash, bitcoin::blockdata::transaction::Transaction>>,
}
impl ChainInterface {
    pub fn new(network: Network, logger: Arc<Logger>) -> Self {
        ChainInterface {
            util: chaininterface::ChainWatchInterfaceUtil::new(network, logger),
            txn_to_broadcast: Mutex::new(HashMap::new()),
        }
    }

    fn rebroadcast_txn(&self) {
        //let mut send_futures = Vec::new();
        {
            let txn = self.txn_to_broadcast.lock().unwrap();
            for (_, tx) in txn.iter() {
                //broadcast without RPC, do we need futures?
//                let tx_ser = "\"".to_string() + &encode::serialize_hex(tx) + "\"";
//                send_futures.push(self.rpc_client.make_rpc_call("sendrawtransaction", &[&tx_ser], true).then(|_| -> Result<(), ()> { Ok(()) }));
            }
        }
        //future::join_all(send_futures).then(|_| -> Result<(), ()> { Ok(()) })
    }
}

impl chaininterface::BroadcasterInterface for ChainInterface {
    fn broadcast_transaction (&self, tx: &bitcoin::blockdata::transaction::Transaction) {
//        self.txn_to_broadcast.lock().unwrap().insert(tx.txid(), tx.clone());
//        let tx_ser = "\"".to_string() + &encode::serialize_hex(tx) + "\"";
//        tokio::spawn(self.rpc_client.make_rpc_call("sendrawtransaction", &[&tx_ser], true).then(|_| { Ok(()) }));

        // SENDRAWTX HERE
    }
}

impl chaininterface::ChainWatchInterface for ChainInterface {
    fn install_watch_tx(&self, txid: &bitcoin_hashes::sha256d::Hash, script: &bitcoin::blockdata::script::Script) {
        self.util.install_watch_tx(txid, script);
    }

    fn install_watch_outpoint(&self, outpoint: (bitcoin_hashes::sha256d::Hash, u32), script_pubkey: &bitcoin::blockdata::script::Script) {
        self.util.install_watch_outpoint(outpoint, script_pubkey);
    }

    fn watch_all_txn(&self) {
        self.util.watch_all_txn();
    }

    fn get_chain_utxo(&self, genesis_hash: bitcoin_hashes::sha256d::Hash, unspent_tx_output_identifier: u64) -> Result<(bitcoin::blockdata::script::Script, u64), ChainError> {
        self.util.get_chain_utxo(genesis_hash, unspent_tx_output_identifier)
    }

    fn filter_block<'a>(&self, block: &'a Block) -> (Vec<&'a Transaction>, Vec<u32>) {
        unimplemented!()
    }

    fn reentered(&self) -> usize {
        unimplemented!()
    }
}

struct ChannelMonitor {
    monitor: Arc<channelmonitor::SimpleManyChannelMonitor<chain::transaction::OutPoint>>,
    file_prefix: String,
}
impl ChannelMonitor {
    fn load_from_disk(file_prefix: &String) -> Vec<(chain::transaction::OutPoint, channelmonitor::ChannelMonitor)> {
        let mut res = Vec::new();
        for file_option in fs::read_dir(file_prefix).unwrap() {
            let mut loaded = false;
            let file = file_option.unwrap();
            if let Some(filename) = file.file_name().to_str() {
                if filename.is_ascii() && filename.len() > 65 {
                    if let Ok(txid) = bitcoin_hashes::sha256d::Hash::from_hex(filename.split_at(64).0) {
                        if let Ok(index) = filename.split_at(65).1.split('.').next().unwrap().parse() {
                            if let Ok(contents) = fs::read(&file.path()) {
                                if let Ok((last_block_hash, loaded_monitor)) = <(bitcoin_hashes::sha256d::Hash, channelmonitor::ChannelMonitor)>::read(&mut Cursor::new(&contents), Arc::new(LogPrinter{})) {
                                    // TODO: Rescan from last_block_hash
                                    res.push((chain::transaction::OutPoint { txid, index }, loaded_monitor));
                                    loaded = true;
                                }
                            }
                        }
                    }
                }
            }
            if !loaded {
                println!("WARNING: Failed to read one of the channel monitor storage files! Check perms!");
            }
        }
        res
    }

    fn load_from_vec(&self, mut monitors: Vec<(chain::transaction::OutPoint, channelmonitor::ChannelMonitor)>) {
        for (outpoint, monitor) in monitors.drain(..) {
            if let Err(_) = self.monitor.add_update_monitor(outpoint, monitor) {
                panic!("Failed to load monitor that deserialized");
            }
        }
    }
}

impl channelmonitor::ManyChannelMonitor for ChannelMonitor {
    fn add_update_monitor(&self, funding_txo: chain::transaction::OutPoint, monitor: channelmonitor::ChannelMonitor) -> Result<(), channelmonitor::ChannelMonitorUpdateErr> {
        macro_rules! try_fs {
			($res: expr) => {
				match $res {
					Ok(res) => res,
					Err(_) => return Err(channelmonitor::ChannelMonitorUpdateErr::PermanentFailure),
				}
			}
		}
        // Do a crazy dance with lots of fsync()s to be overly cautious here...
        // We never want to end up in a state where we've lost the old data, or end up using the
        // old data on power loss after we've returned
        // Note that this actually *isn't* enough (at least on Linux)! We need to fsync an fd with
        // the containing dir, but Rust doesn't let us do that directly, sadly. TODO: Fix this with
        // the libc crate!
        let filename = format!("{}/{}_{}", self.file_prefix, funding_txo.txid.to_hex(), funding_txo.index);
        let tmp_filename = filename.clone() + ".tmp";

        {
            let mut f = try_fs!(fs::File::create(&tmp_filename));
            try_fs!(monitor.write_for_disk(&mut f));
            try_fs!(f.sync_all());
        }
        // We don't need to create a backup if didn't already have the file, but in any other case
        // try to create the backup and expect failure on fs::copy() if eg there's a perms issue.
        let need_bk = match fs::metadata(&filename) {
            Ok(data) => {
                if !data.is_file() { return Err(channelmonitor::ChannelMonitorUpdateErr::PermanentFailure); }
                true
            },
            Err(e) => match e.kind() {
                std::io::ErrorKind::NotFound => false,
                _ => true,
            }
        };
        let bk_filename = filename.clone() + ".bk";
        if need_bk {
            try_fs!(fs::copy(&filename, &bk_filename));
            {
                let f = try_fs!(fs::File::open(&bk_filename));
                try_fs!(f.sync_all());
            }
        }
        try_fs!(fs::rename(&tmp_filename, &filename));
        {
            let f = try_fs!(fs::File::open(&filename));
            try_fs!(f.sync_all());
        }
        if need_bk {
            try_fs!(fs::remove_file(&bk_filename));
        }
        self.monitor.add_update_monitor(funding_txo, monitor)
    }

    fn fetch_pending_htlc_updated(&self) -> Vec<channelmonitor::HTLCUpdate> {
        self.monitor.fetch_pending_htlc_updated()
    }
}