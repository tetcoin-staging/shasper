// Copyright 2019 Parity Technologies (UK) Ltd.
// Copyright 2019 Sigma Prime.
// This file is part of Parity Shasper.

// Parity Shasper is free software: you can redistribute it and/or modify it
// under the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option) any
// later version.

// Parity Shasper is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE.  See the GNU General Public License for more
// details.

// You should have received a copy of the GNU General Public License along with
// Parity Shasper.  If not, see <http://www.gnu.org/licenses/>.

use crate::rpc::methods::*;
use crate::rpc::{
    codec::base::OutboundCodec,
    protocol::{ProtocolId, RPCError},
};
use crate::rpc::{ErrorMessage, RPCErrorResponse, RPCRequest, RPCResponse};
use bytes::{Bytes, BytesMut};
use ssz::{Decode, Encode};
use tokio::codec::{Decoder, Encoder};
use unsigned_varint::codec::UviBytes;

pub struct SSZInboundCodec {
    inner: UviBytes,
    protocol: ProtocolId,
}

impl SSZInboundCodec {
    pub fn new(protocol: ProtocolId, max_packet_size: usize) -> Self {
        let mut uvi_codec = UviBytes::default();
        uvi_codec.set_max_len(max_packet_size);

        // this encoding only applies to ssz.
        debug_assert!(protocol.encoding.as_str() == "ssz");

        SSZInboundCodec {
            inner: uvi_codec,
            protocol,
        }
    }
}

// Encoder for inbound
impl Encoder for SSZInboundCodec {
    type Item = RPCErrorResponse;
    type Error = RPCError;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = match item {
            RPCErrorResponse::Success(resp) => {
                match resp {
                    RPCResponse::Hello(res) => res.encode(),
                    RPCResponse::BeaconBlocks(res) => res, // already raw bytes
                    RPCResponse::RecentBeaconBlocks(res) => res, // already raw bytes
                }
            }
            RPCErrorResponse::InvalidRequest(err) => err.encode(),
            RPCErrorResponse::ServerError(err) => err.encode(),
            RPCErrorResponse::Unknown(err) => err.encode(),
        };

        if !bytes.is_empty() {
            // length-prefix and return
            return self
                .inner
                .encode(Bytes::from(bytes), dst)
                .map_err(RPCError::from);
        }
        Ok(())
    }
}

// Decoder for inbound
impl Decoder for SSZInboundCodec {
    type Item = RPCRequest;
    type Error = RPCError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.inner.decode(src).map_err(RPCError::from) {
            Ok(Some(packet)) => match self.protocol.message_name.as_str() {
                "hello" => match self.protocol.version.as_str() {
                    "1" => Ok(Some(RPCRequest::Hello(HelloMessage::decode(
                        &packet,
                    )?))),
                    _ => unreachable!("Cannot negotiate an unknown version"),
                },
                "goodbye" => match self.protocol.version.as_str() {
                    "1" => Ok(Some(RPCRequest::Goodbye(GoodbyeReason::decode(
                        &packet,
                    )?))),
                    _ => unreachable!("Cannot negotiate an unknown version"),
                },
                "beacon_blocks" => match self.protocol.version.as_str() {
                    "1" => Ok(Some(RPCRequest::BeaconBlocks(
                        BeaconBlocksRequest::decode(&packet)?,
                    ))),
                    _ => unreachable!("Cannot negotiate an unknown version"),
                },
                "recent_beacon_blocks" => match self.protocol.version.as_str() {
                    "1" => Ok(Some(RPCRequest::RecentBeaconBlocks(
                        RecentBeaconBlocksRequest::decode(&packet)?,
                    ))),
                    _ => unreachable!("Cannot negotiate an unknown version"),
                },
                _ => unreachable!("Cannot negotiate an unknown protocol"),
            },
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/* Outbound Codec */

pub struct SSZOutboundCodec {
    inner: UviBytes,
    protocol: ProtocolId,
}

impl SSZOutboundCodec {
    pub fn new(protocol: ProtocolId, max_packet_size: usize) -> Self {
        let mut uvi_codec = UviBytes::default();
        uvi_codec.set_max_len(max_packet_size);

        // this encoding only applies to ssz.
        debug_assert!(protocol.encoding.as_str() == "ssz");

        SSZOutboundCodec {
            inner: uvi_codec,
            protocol,
        }
    }
}

// Encoder for outbound
impl Encoder for SSZOutboundCodec {
    type Item = RPCRequest;
    type Error = RPCError;

    fn encode(&mut self, item: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = match item {
            RPCRequest::Hello(req) => req.encode(),
            RPCRequest::Goodbye(req) => req.encode(),
            RPCRequest::BeaconBlocks(req) => req.encode(),
            RPCRequest::RecentBeaconBlocks(req) => req.encode(),
        };
        // length-prefix
        self.inner
            .encode(bytes::Bytes::from(bytes), dst)
            .map_err(RPCError::from)
    }
}

// Decoder for outbound streams
//
// The majority of the decoding has now been pushed upstream due to the changing specification.
// We prefer to decode blocks and attestations with extra knowledge about the chain to perform
// faster verification checks before decoding entire blocks/attestations.
impl Decoder for SSZOutboundCodec {
    type Item = RPCResponse;
    type Error = RPCError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.inner.decode(src).map_err(RPCError::from) {
            Ok(Some(packet)) => match self.protocol.message_name.as_str() {
                "hello" => match self.protocol.version.as_str() {
                    "1" => Ok(Some(RPCResponse::Hello(HelloMessage::decode(
                        &packet,
                    )?))),
                    _ => unreachable!("Cannot negotiate an unknown version"),
                },
                "goodbye" => Err(RPCError::InvalidProtocol("GOODBYE doesn't have a response")),
                "beacon_blocks" => match self.protocol.version.as_str() {
                    "1" => Ok(Some(RPCResponse::BeaconBlocks(packet.to_vec()))),
                    _ => unreachable!("Cannot negotiate an unknown version"),
                },
                "recent_beacon_blocks" => match self.protocol.version.as_str() {
                    "1" => Ok(Some(RPCResponse::RecentBeaconBlocks(packet.to_vec()))),
                    _ => unreachable!("Cannot negotiate an unknown version"),
                },
                _ => unreachable!("Cannot negotiate an unknown protocol"),
            },
            Ok(None) => {
                // the object sent could be a empty. We return the empty object if this is the case
                match self.protocol.message_name.as_str() {
                    "hello" => match self.protocol.version.as_str() {
                        "1" => Ok(None), // cannot have an empty HELLO message. The stream has terminated unexpectedly
                        _ => unreachable!("Cannot negotiate an unknown version"),
                    },
                    "goodbye" => Err(RPCError::InvalidProtocol("GOODBYE doesn't have a response")),
                    "beacon_blocks" => match self.protocol.version.as_str() {
                        "1" => Ok(Some(RPCResponse::BeaconBlocks(Vec::new()))),
                        _ => unreachable!("Cannot negotiate an unknown version"),
                    },
                    "recent_beacon_blocks" => match self.protocol.version.as_str() {
                        "1" => Ok(Some(RPCResponse::RecentBeaconBlocks(Vec::new()))),
                        _ => unreachable!("Cannot negotiate an unknown version"),
                    },
                    _ => unreachable!("Cannot negotiate an unknown protocol"),
                }
            }
            Err(e) => Err(e),
        }
    }
}

impl OutboundCodec for SSZOutboundCodec {
    type ErrorType = ErrorMessage;

    fn decode_error(&mut self, src: &mut BytesMut) -> Result<Option<Self::ErrorType>, RPCError> {
        match self.inner.decode(src).map_err(RPCError::from) {
            Ok(Some(packet)) => Ok(Some(ErrorMessage::decode(&packet)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
