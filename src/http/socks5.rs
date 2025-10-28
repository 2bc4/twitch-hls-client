use std::{
    io::{Read, Write},
    net::TcpStream,
};

use anyhow::{Result, ensure};

const SOCKS_VERSION: u8 = 0x05;
const NO_AUTH_NUM_METHODS: u8 = 0x01;
const NO_AUTH: u8 = 0x00;
const HANDSHAKE: [u8; 3] = [SOCKS_VERSION, NO_AUTH_NUM_METHODS, NO_AUTH];

const CONNECT_COMMAND: u8 = 0x01;
const ADDRESS_TYPE_DOMAIN: u8 = 0x03;
const RESERVED: u8 = 0x00;

const COMMAND_SUCCESS: u8 = 0x00;

const HANDSHAKE_RESPONSE_LEN: usize = 2;
const REQUEST_RESPONSE_LEN: usize = 10;

pub fn connect(mut sock: TcpStream, target_host: &str, target_port: u16) -> Result<TcpStream> {
    sock.write_all(&HANDSHAKE)?;

    let mut response = [0u8; HANDSHAKE_RESPONSE_LEN];
    sock.read_exact(&mut response)?;
    ensure!(
        response[0] == SOCKS_VERSION && response[1] == NO_AUTH,
        "Invalid handshake from SOCKS5 server"
    );

    let mut request = vec![
        SOCKS_VERSION,
        CONNECT_COMMAND,
        RESERVED,
        ADDRESS_TYPE_DOMAIN,
        u8::try_from(target_host.len())?,
    ];
    request.extend_from_slice(target_host.as_bytes());
    request.extend_from_slice(&target_port.to_be_bytes());
    sock.write_all(&request)?;

    //Only the first 2 bytes are needed
    let mut response = [0u8; REQUEST_RESPONSE_LEN];
    sock.read_exact(&mut response)?;
    ensure!(
        response[0] == SOCKS_VERSION && response[1] == COMMAND_SUCCESS,
        "SOCKS5 request failed: {:X}",
        response[1]
    );

    Ok(sock)
}
