use std::{
    io::{Read, Write},
    mem::size_of,
    net::TcpStream,
};

use anyhow::{Result, bail, ensure};

use super::{MAX_HEADERS_SIZE, parse_status};

pub fn http_connect(mut sock: TcpStream, target_host: &str, target_port: u16) -> Result<TcpStream> {
    write!(
        sock,
        "CONNECT {target_host}:{target_port} HTTP/1.1\r\n\
         Host: {target_host}:{target_port}\r\n\
         \r\n"
    )?;

    let mut buf = [0u8; MAX_HEADERS_SIZE];
    let mut written = 0usize;
    let headers = loop {
        //Read one byte per loop to avoid consuming data after headers
        let mut byte = [0u8; 1];
        sock.read_exact(&mut byte)?;

        buf[written] = byte[0];
        written += 1;
        if buf[..written].ends_with(b"\r\n\r\n") {
            break str::from_utf8(&buf[..written])?;
        }

        ensure!(
            written < MAX_HEADERS_SIZE,
            "HTTP proxy response exceeded {MAX_HEADERS_SIZE} bytes"
        );
    };

    let status = parse_status(headers)?;
    ensure!(
        status == 200,
        "HTTP proxy CONNECT failed with status: {status}"
    );

    Ok(sock)
}

pub fn socks5_connect(
    mut sock: TcpStream,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream> {
    const SOCKS_VERSION: u8 = 0x05;
    const NO_AUTH_NUM_METHODS: u8 = 0x01;
    const NO_AUTH: u8 = 0x00;
    const HANDSHAKE: [u8; 3] = [SOCKS_VERSION, NO_AUTH_NUM_METHODS, NO_AUTH];

    const ADDRESS_TYPE_DOMAIN: u8 = 0x03;
    const ADDRESS_TYPE_IPV4: u8 = 0x01;
    const ADDRESS_TYPE_IPV6: u8 = 0x04;

    const CONNECT_COMMAND: u8 = 0x01;
    const COMMAND_SUCCESS: u8 = 0x00;
    const RESERVED: u8 = 0x00;

    const HANDSHAKE_RESPONSE_LEN: usize = 2;
    const RESPONSE_HEADER_LEN: usize = 4;
    const RESPONSE_IPV4_LEN: usize = size_of::<u32>() + size_of::<u16>();
    const RESPONSE_IPV6_LEN: usize = size_of::<u128>() + size_of::<u16>();

    sock.write_all(&HANDSHAKE)?;

    let mut response = [0u8; HANDSHAKE_RESPONSE_LEN];
    sock.read_exact(&mut response)?;
    ensure!(
        response[0] == SOCKS_VERSION && response[1] == NO_AUTH,
        "Invalid handshake from socks5 server"
    );

    sock.write_all(&[
        SOCKS_VERSION,
        CONNECT_COMMAND,
        RESERVED,
        ADDRESS_TYPE_DOMAIN,
        u8::try_from(target_host.len())?,
    ])?;
    sock.write_all(target_host.as_bytes())?;
    sock.write_all(&target_port.to_be_bytes())?;

    //Response: VER(1) + REP(1) + RSV(1) + ATYP(1) + address/domain(N) + port(2)
    let mut response = [0u8; RESPONSE_HEADER_LEN];
    sock.read_exact(&mut response)?;

    ensure!(
        response[0] == SOCKS_VERSION && response[1] == COMMAND_SUCCESS,
        "socks5 request failed with error: {:X}",
        response[1]
    );

    match response[RESPONSE_HEADER_LEN - 1] {
        ADDRESS_TYPE_IPV4 => {
            let mut drain = [0u8; RESPONSE_IPV4_LEN];
            sock.read_exact(&mut drain)?;
        }
        ADDRESS_TYPE_IPV6 => {
            let mut drain = [0u8; RESPONSE_IPV6_LEN];
            sock.read_exact(&mut drain)?;
        }
        ADDRESS_TYPE_DOMAIN => {
            let mut domain_len = [0u8; 1];
            sock.read_exact(&mut domain_len)?;

            let mut drain = vec![0u8; domain_len[0] as usize + size_of::<u16>()];
            sock.read_exact(&mut drain)?;
        }
        _ => bail!("Invalid address type in socks5 response"),
    }

    Ok(sock)
}
