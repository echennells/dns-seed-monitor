use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Mainnet,
    Testnet,
    Signet,
}

impl Network {
    pub fn p2p_port(self) -> u16 {
        match self {
            Network::Mainnet => 8333,
            Network::Testnet => 18333,
            Network::Signet => 38333,
        }
    }

    pub fn magic(self) -> [u8; 4] {
        match self {
            Network::Mainnet => [0xf9, 0xbe, 0xb4, 0xd9],
            Network::Testnet => [0x0b, 0x11, 0x09, 0x07],
            Network::Signet => [0x0a, 0x03, 0xcf, 0x40],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Seed {
    pub hostname: &'static str,
    pub network: Network,
    pub operator: &'static str,
}

/// All DNS seeds across mainnet, testnet (testnet3), and signet, mirrored
/// from Bitcoin Core's `kernel/chainparams.cpp` `vSeeds` blocks.
pub const ALL_SEEDS: &[Seed] = &[
    // ── Mainnet ────────────────────────────────────────────────────
    Seed { hostname: "seed.bitcoin.sipa.be",                       network: Network::Mainnet, operator: "Pieter Wuille" },
    Seed { hostname: "dnsseed.bluematt.me",                        network: Network::Mainnet, operator: "Matt Corallo" },
    Seed { hostname: "dnsseed.bitcoin.dashjr-list-of-p2p-nodes.us",network: Network::Mainnet, operator: "Luke Dashjr" },
    Seed { hostname: "seed.bitcoinstats.com",                      network: Network::Mainnet, operator: "Christian Decker" },
    Seed { hostname: "seed.bitcoin.jonasschnelli.ch",              network: Network::Mainnet, operator: "Jonas Schnelli" },
    Seed { hostname: "seed.btc.petertodd.net",                     network: Network::Mainnet, operator: "Peter Todd" },
    Seed { hostname: "seed.bitcoin.sprovoost.nl",                  network: Network::Mainnet, operator: "Sjors Provoost" },
    Seed { hostname: "dnsseed.emzy.de",                            network: Network::Mainnet, operator: "Stephan Oeste" },
    Seed { hostname: "seed.bitcoin.wiz.biz",                       network: Network::Mainnet, operator: "Jason Maurice" },
    Seed { hostname: "seed.mainnet.achownodes.xyz",                network: Network::Mainnet, operator: "Ava Chow" },

    // ── Testnet (testnet3) ─────────────────────────────────────────
    Seed { hostname: "testnet-seed.bitcoin.jonasschnelli.ch", network: Network::Testnet, operator: "Jonas Schnelli" },
    Seed { hostname: "seed.tbtc.petertodd.net",               network: Network::Testnet, operator: "Peter Todd" },
    Seed { hostname: "seed.testnet.bitcoin.sprovoost.nl",     network: Network::Testnet, operator: "Sjors Provoost" },
    Seed { hostname: "testnet-seed.bluematt.me",              network: Network::Testnet, operator: "Matt Corallo" },
    Seed { hostname: "seed.testnet.achownodes.xyz",           network: Network::Testnet, operator: "Ava Chow" },

    // ── Signet ─────────────────────────────────────────────────────
    Seed { hostname: "seed.signet.bitcoin.sprovoost.nl", network: Network::Signet, operator: "Sjors Provoost" },
    Seed { hostname: "seed.signet.achownodes.xyz",       network: Network::Signet, operator: "Ava Chow" },
];

pub fn all() -> &'static [Seed] {
    ALL_SEEDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_nonempty_and_unique() {
        let s = all();
        assert!(s.len() >= 15);
        let mut hosts: Vec<_> = s.iter().map(|x| x.hostname).collect();
        hosts.sort();
        let len = hosts.len();
        hosts.dedup();
        assert_eq!(hosts.len(), len, "duplicate seed hostnames");
    }

    #[test]
    fn all_three_networks_present() {
        let s = all();
        assert!(s.iter().any(|x| x.network == Network::Mainnet));
        assert!(s.iter().any(|x| x.network == Network::Testnet));
        assert!(s.iter().any(|x| x.network == Network::Signet));
    }
}
