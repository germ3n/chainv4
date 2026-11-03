use {
    super::{MAX_SYNC_RESPONSE as MAX_RESPONSE, SyncRequest, SyncResponse},
    libp2p::StreamProtocol,
    libp2p::futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    libp2p::request_response::Codec,
    std::io,
};

/// The most bytes read for one request. Requests are tiny; anything bigger is hostile.
const MAX_REQUEST: u64 = 1024;

/// Sync messages as wincode, one per stream, read up to a size limit so a peer cannot make this
/// node buffer without end.
#[derive(Clone, Default)]
pub struct SyncCodec;

async fn read<T, M>(io: &mut T, limit: u64) -> io::Result<M>
where
    T: AsyncRead + Unpin + Send,
    M: for<'de> wincode::SchemaRead<'de, wincode::config::DefaultConfig, Dst = M>,
{
    let mut bytes = Vec::new();
    io.take(limit + 1).read_to_end(&mut bytes).await?;

    if bytes.len() as u64 > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "sync message too large"));
    }

    wincode::deserialize(&bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

async fn write<T, M>(io: &mut T, message: &M) -> io::Result<()>
where
    T: AsyncWrite + Unpin + Send,
    M: wincode::SchemaWrite<wincode::config::DefaultConfig, Src = M>,
{
    let bytes =
        wincode::serialize(message).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    io.write_all(&bytes).await?;
    io.close().await
}

impl Codec for SyncCodec {
    type Protocol = StreamProtocol;
    type Request = SyncRequest;
    type Response = SyncResponse;

    async fn read_request<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<SyncRequest>
    where
        T: AsyncRead + Unpin + Send,
    {
        read(io, MAX_REQUEST).await
    }

    async fn read_response<T>(&mut self, _: &StreamProtocol, io: &mut T) -> io::Result<SyncResponse>
    where
        T: AsyncRead + Unpin + Send,
    {
        read(io, MAX_RESPONSE).await
    }

    async fn write_request<T>(&mut self, _: &StreamProtocol, io: &mut T, request: SyncRequest) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write(io, &request).await
    }

    async fn write_response<T>(&mut self, _: &StreamProtocol, io: &mut T, response: SyncResponse) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write(io, &response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::hash::HashType;
    use crate::structs::hash::Hash;
    use libp2p::futures::executor::block_on;
    use libp2p::futures::io::Cursor;

    const PROTOCOL: StreamProtocol = StreamProtocol::new("/test");

    #[test]
    fn messages_round_trip() {
        let mut codec = SyncCodec;
        let request = SyncRequest::Blocks { from: 7, count: 3 };
        let response = SyncResponse::Status {
            genesis: Hash::from_bytes(HashType::default_chain(), &[1; 32]).unwrap(),
            tip: Hash::from_bytes(HashType::default_chain(), &[2; 32]).unwrap(),
            height: 9,
            work: crate::structs::u256::u256::new(7),
            justified: 3,
        };
        let mut wire = Cursor::new(Vec::new());
        block_on(codec.write_request(&PROTOCOL, &mut wire, request.clone())).unwrap();
        wire.set_position(0);

        assert_eq!(block_on(codec.read_request(&PROTOCOL, &mut wire)).unwrap(), request);

        let mut wire = Cursor::new(Vec::new());
        block_on(codec.write_response(&PROTOCOL, &mut wire, response.clone())).unwrap();
        wire.set_position(0);

        assert_eq!(block_on(codec.read_response(&PROTOCOL, &mut wire)).unwrap(), response);
    }

    #[test]
    fn oversized_requests_are_refused() {
        let mut wire = Cursor::new(vec![0; MAX_REQUEST as usize + 10]);

        assert!(block_on(SyncCodec.read_request(&PROTOCOL, &mut wire)).is_err());
    }
}
