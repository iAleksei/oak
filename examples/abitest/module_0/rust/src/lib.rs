//
// Copyright 2019 The Project Oak Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

pub mod proto;

use abitest_common::{InternalMessage, LOG_CONFIG_NAME};
use byteorder::WriteBytesExt;
use expect::{expect, expect_eq, expect_matches};
use log::{debug, info};
use oak::{grpc, ChannelReadStatus, OakError, OakStatus};
use proto::abitest::GrpcTestRequest_oneof_method_result::{err_code, ok_text};
use proto::abitest::{ABITestRequest, ABITestResponse, ABITestResponse_TestResult};
use proto::abitest::{GrpcTestRequest, GrpcTestResponse};
use proto::abitest_grpc::{Dispatcher, OakABITestService, OakABITestServiceClient};
use protobuf::{Message, ProtobufEnum};
use rand::Rng;
use std::collections::HashMap;

const BACKEND_COUNT: usize = 3;

const BACKEND_CONFIG_NAME: &str = "backend-config";
const BACKEND_ENTRYPOINT_NAME: &str = "backend_oak_main";
const STORAGE_NAME_PREFIX: &str = "abitest";

struct FrontendNode {
    backend_out: Vec<oak::WriteHandle>,
    backend_in: Vec<oak::ReadHandle>,
    storage: Option<oak::storage::Storage>,
    absent_storage: Option<oak::storage::Storage>,
    storage_name: Vec<u8>,
    grpc_service: Option<OakABITestServiceClient>,
    absent_grpc_service: Option<OakABITestServiceClient>,
}

impl FrontendNode {
    pub fn new() -> Self {
        oak::logger::init(log::Level::Debug, LOG_CONFIG_NAME)
            .expect("could not initialize logging node");

        // Create backend node instances.
        let mut backend_out = Vec::with_capacity(BACKEND_COUNT);
        let mut backend_in = Vec::with_capacity(BACKEND_COUNT);
        for i in 0..BACKEND_COUNT {
            let (write_handle, read_handle) =
                oak::channel_create().expect("could not create channel");
            oak::node_create(BACKEND_CONFIG_NAME, BACKEND_ENTRYPOINT_NAME, read_handle)
                .expect("could not create node");
            oak::channel_close(read_handle.handle).expect("could not close channel");
            backend_out.push(write_handle);

            // Create a back channel, and pass the write half to the backend
            // as the first message on the outbound channel.
            let (write_handle, read_handle) =
                oak::channel_create().expect("could not create channel");
            oak::channel_write(backend_out[i], &[], &[write_handle.handle])
                .expect("could not write to channel");
            oak::channel_close(write_handle.handle).expect("could not close channel");
            backend_in.push(read_handle);
        }

        // Build a unique storage name, so different test runs don't affect each other.
        let mut nonce = [0; 16];
        oak::random_get(&mut nonce).unwrap();
        let storage_name = format!("{}-{}", STORAGE_NAME_PREFIX, hex::encode(nonce));
        info!("Using storage name '{}' for storage tests", storage_name);

        FrontendNode {
            backend_out,
            backend_in,
            storage: oak::storage::Storage::default(),
            absent_storage: oak::storage::Storage::new("absent-storage"),
            storage_name: storage_name.as_bytes().to_vec(),
            grpc_service: oak::grpc::client::Client::new("grpc-client", "ignored")
                .map(OakABITestServiceClient),
            absent_grpc_service: oak::grpc::client::Client::new("absent-grpc-client", "ignored")
                .map(OakABITestServiceClient),
        }
    }
}

#[no_mangle]
pub extern "C" fn frontend_oak_main(in_handle: u64) {
    let _ = std::panic::catch_unwind(|| {
        oak::set_panic_hook();
        let node = FrontendNode::new();
        let dispatcher = Dispatcher::new(node);
        oak::run_event_loop(dispatcher, in_handle);
    });
}

type TestResult = Result<(), Box<dyn std::error::Error>>;
type TestFn = fn(&mut FrontendNode) -> TestResult;

impl OakABITestService for FrontendNode {
    fn run_tests(&mut self, req: ABITestRequest) -> grpc::Result<ABITestResponse> {
        info!(
            "Run tests matching '{}' except those matching '{}'",
            req.include, req.exclude
        );
        let include = regex::Regex::new(&req.include).unwrap();
        let exclude = regex::Regex::new(&req.exclude).unwrap();
        let mut results = protobuf::RepeatedField::<ABITestResponse_TestResult>::new();

        // Manual registry of all tests.
        let mut tests: HashMap<&str, TestFn> = HashMap::new();
        tests.insert("ChannelCreateRaw", FrontendNode::test_channel_create_raw);
        tests.insert("ChannelCreate", FrontendNode::test_channel_create);
        tests.insert("ChannelCloseRaw", FrontendNode::test_channel_close_raw);
        tests.insert("ChannelClose", FrontendNode::test_channel_close);
        tests.insert("ChannelReadRaw", FrontendNode::test_channel_read_raw);
        tests.insert("ChannelRead", FrontendNode::test_channel_read);
        tests.insert("ChannelReadOrphan", FrontendNode::test_channel_read_orphan);
        tests.insert("ChannelWriteRaw", FrontendNode::test_channel_write_raw);
        tests.insert("ChannelWrite", FrontendNode::test_channel_write);
        tests.insert(
            "ChannelWriteOrphan",
            FrontendNode::test_channel_write_orphan,
        );
        tests.insert("WaitOnChannelsRaw", FrontendNode::test_channel_wait_raw);
        tests.insert("WaitOnChannels", FrontendNode::test_channel_wait);
        tests.insert(
            "WaitOnChannelsOrphan",
            FrontendNode::test_channel_wait_orphan,
        );
        tests.insert("NodeCreate", FrontendNode::test_node_create);
        tests.insert("NodeCreateRaw", FrontendNode::test_node_create_raw);
        tests.insert("RandomGetRaw", FrontendNode::test_random_get_raw);
        tests.insert("RandomGet", FrontendNode::test_random_get);
        tests.insert("RandomRng", FrontendNode::test_random_rng);
        tests.insert(
            "ChannelHandleReuse",
            FrontendNode::test_channel_handle_reuse,
        );
        tests.insert("DirectLog", FrontendNode::test_direct_log);
        tests.insert("BackendRoundtrip", FrontendNode::test_backend_roundtrip);
        tests.insert("Storage", FrontendNode::test_storage);
        tests.insert("AbsentStorage", FrontendNode::test_absent_storage);
        tests.insert(
            "GrpcClientUnaryMethod",
            FrontendNode::test_grpc_client_unary_method,
        );
        tests.insert(
            "GrpcClientServerStreamingMethod",
            FrontendNode::test_grpc_client_server_streaming_method,
        );
        tests.insert("AbsentGrpcClient", FrontendNode::test_absent_grpc_client);

        for (&name, &testfn) in &tests {
            if !include.is_match(name) {
                debug!(
                    "skip test '{}' as not included in '{}' include pattern",
                    name, include
                );
                continue;
            }
            if !req.exclude.is_empty() && exclude.is_match(name) {
                debug!(
                    "skip test '{}' as included in '{}' exclude pattern",
                    name, exclude
                );
                continue;
            }
            let mut result = ABITestResponse_TestResult::new();
            result.set_name(name.to_string());
            if name.starts_with("DISABLED_") {
                debug!("skip test '{}' as marked as disabled", name);
                result.set_disabled(true);
                result.set_success(false);
                result.set_details("Test disabled".to_string());
                results.push(result);
                continue;
            }
            info!("running test {}", name);
            match testfn(self) {
                Ok(()) => result.set_success(true),
                Err(err) => {
                    result.set_success(false);
                    result.set_details(format!("Error: {}", err));
                }
            }
            results.push(result);
        }

        let mut res = ABITestResponse::new();
        res.set_results(results);

        Ok(res)
    }

    // gRPC test methods.
    fn unary_method(&mut self, req: GrpcTestRequest) -> grpc::Result<GrpcTestResponse> {
        if req.has_err_code() {
            info!("unary_method -> Err({})", req.get_err_code());
            return Err(grpc::build_status(
                grpc::Code::from_i32(req.get_err_code()).unwrap(),
                "Deliberate error",
            ));
        }
        info!("unary_method -> Ok({})", req.get_ok_text());
        let mut rsp = GrpcTestResponse::new();
        rsp.text = req.get_ok_text().to_string();
        Ok(rsp)
    }
    fn server_streaming_method(
        &mut self,
        req: GrpcTestRequest,
        mut writer: grpc::ChannelResponseWriter,
    ) {
        if req.has_err_code() {
            info!("server_streaming_method -> Err({})", req.get_err_code());
            let status = grpc::build_status(
                grpc::Code::from_i32(req.get_err_code()).unwrap(),
                "Deliberate error",
            );
            writer.close(Err(status)).expect("failed to close writer");
            return;
        }
        // Write two responses to exercise streaming.
        info!("server_streaming_method -> 2 x Ok({})", req.get_ok_text());
        let mut rsp = GrpcTestResponse::new();
        rsp.set_text(req.get_ok_text().to_string());
        writer
            .write(&rsp, grpc::WriteMode::KeepOpen)
            .expect("Failed to write response");
        rsp.set_text(req.get_ok_text().to_string());
        writer
            .write(&rsp, grpc::WriteMode::Close)
            .expect("Failed to write response");
    }
    fn client_streaming_method(
        &mut self,
        reqs: Vec<GrpcTestRequest>,
    ) -> grpc::Result<GrpcTestResponse> {
        // If any request triggers an error, return it.
        for req in &reqs {
            if req.has_err_code() {
                info!("client_streaming_method -> Err({})", req.get_err_code());
                return Err(grpc::build_status(
                    grpc::Code::from_i32(req.get_err_code()).unwrap(),
                    "Deliberate error",
                ));
            }
        }
        // Otherwise return the text from all the requests combined.
        let mut combined_text = String::new();
        for req in &reqs {
            combined_text.push_str(req.get_ok_text());
        }
        info!("client_streaming_method -> Ok({})", combined_text);
        let mut rsp = GrpcTestResponse::new();
        rsp.text = combined_text;
        Ok(rsp)
    }
    fn bidi_streaming_method(
        &mut self,
        reqs: Vec<GrpcTestRequest>,
        mut writer: grpc::ChannelResponseWriter,
    ) {
        for req in &reqs {
            if req.has_err_code() {
                info!("bidi_streaming_method -> Err({})", req.get_err_code());
                let status = grpc::build_status(
                    grpc::Code::from_i32(req.get_err_code()).unwrap(),
                    "Deliberate error",
                );
                writer.close(Err(status)).expect("failed to close writer");
                return;
            }
            info!("bidi_streaming_method -> Ok({})", req.get_ok_text());
            let mut rsp = GrpcTestResponse::new();
            rsp.set_text(req.get_ok_text().to_string());
            writer
                .write(&rsp, grpc::WriteMode::KeepOpen)
                .expect("Failed to write response");
        }
    }
}

// Helper for status conversion
fn status_convert<T>(result: Result<T, OakStatus>) -> Result<T, OakError> {
    match result {
        Ok(t) => Ok(t),
        Err(status) => Err(OakError::OakStatus(status)),
    }
}

// Return an offset value that is beyond the bounds of available linear memory.
unsafe fn invalid_raw_offset() -> *mut u64 {
    // Currently no way to get at the `memory.size` Wasm instruction from Rust,
    // so pick a large number instead.
    0x7fff_fff0 as *mut u64
}

// Helper function to simplify creating nodes through oak_abi::channel_create
fn channel_create_raw() -> (u64, u64, u32) {
    let mut w = 0u64;
    let mut r = 0u64;
    let result = unsafe { oak_abi::channel_create(&mut w as *mut u64, &mut r as *mut u64) };
    (w, r, result)
}

impl FrontendNode {
    fn test_channel_create_raw(&mut self) -> TestResult {
        let mut write = 0u64;
        let mut read = 0u64;
        unsafe {
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_create(invalid_raw_offset(), &mut read as *mut u64)
            );
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_create(&mut write as *mut u64, invalid_raw_offset())
            );
        }
        Ok(())
    }

    fn test_channel_create(&mut self) -> TestResult {
        let mut handles = Vec::<(oak::WriteHandle, oak::ReadHandle)>::new();
        const CHANNEL_COUNT: usize = 50;
        for _ in 0..CHANNEL_COUNT {
            match oak::channel_create() {
                Ok(pair) => handles.push(pair),
                Err(status) => {
                    return Err(Box::new(OakError::OakStatus(status)));
                }
            }
        }
        for (w, r) in handles {
            expect_eq!(Ok(()), oak::channel_close(r.handle));
            expect_eq!(Ok(()), oak::channel_close(w.handle));
        }
        Ok(())
    }

    fn test_channel_close_raw(&mut self) -> TestResult {
        let (w, r, _) = channel_create_raw();

        unsafe {
            expect_eq!(OakStatus::Ok as u32, oak_abi::channel_close(w));
            expect_eq!(OakStatus::Ok as u32, oak_abi::channel_close(r));
            expect_eq!(OakStatus::ErrBadHandle as u32, oak_abi::channel_close(w));
            expect_eq!(
                OakStatus::ErrBadHandle as u32,
                oak_abi::channel_close(9_999_999)
            );
        }
        Ok(())
    }

    fn test_channel_close(&mut self) -> TestResult {
        let (w, r) = oak::channel_create().unwrap();
        expect_eq!(Ok(()), oak::channel_close(w.handle));
        expect_eq!(Ok(()), oak::channel_close(r.handle));
        expect_eq!(Err(OakStatus::ErrBadHandle), oak::channel_close(w.handle));
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::channel_close(oak::Handle::from_raw(9_999_999))
        );

        // Can close ends in either order.
        let (w, r) = oak::channel_create().unwrap();
        expect_eq!(Ok(()), oak::channel_close(r.handle));
        expect_eq!(Ok(()), oak::channel_close(w.handle));
        Ok(())
    }

    fn test_channel_read_raw(&mut self) -> TestResult {
        let (out_channel, in_channel, _) = channel_create_raw();

        let mut buf = Vec::<u8>::with_capacity(5);
        let mut handles = Vec::with_capacity(5);
        let mut actual_size: u32 = 99;
        let mut actual_handle_count: u32 = 99;
        unsafe {
            // Try invalid values for the 4 linear memory offset arguments.
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_read(
                    in_channel,
                    invalid_raw_offset() as *mut u8,
                    1,
                    &mut actual_size,
                    handles.as_mut_ptr() as *mut u8,
                    handles.capacity() as u32,
                    &mut actual_handle_count
                )
            );
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_read(
                    in_channel,
                    buf.as_mut_ptr(),
                    buf.capacity(),
                    invalid_raw_offset() as *mut u32,
                    handles.as_mut_ptr() as *mut u8,
                    handles.capacity() as u32,
                    &mut actual_handle_count
                )
            );
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_read(
                    in_channel,
                    buf.as_mut_ptr(),
                    buf.capacity(),
                    &mut actual_size,
                    invalid_raw_offset() as *mut u8,
                    1,
                    &mut actual_handle_count
                )
            );
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_read(
                    in_channel,
                    buf.as_mut_ptr(),
                    buf.capacity(),
                    &mut actual_size,
                    handles.as_mut_ptr() as *mut u8,
                    handles.capacity() as u32,
                    invalid_raw_offset() as *mut u32
                )
            );

            // Valid case.
            expect_eq!(
                OakStatus::ErrChannelEmpty as u32,
                oak_abi::channel_read(
                    in_channel,
                    buf.as_mut_ptr(),
                    buf.capacity(),
                    &mut actual_size,
                    handles.as_mut_ptr() as *mut u8,
                    handles.capacity() as u32,
                    &mut actual_handle_count
                )
            );
            expect_eq!(0, actual_size);
            expect_eq!(0, actual_handle_count);
        }
        expect_eq!(OakStatus::Ok as u32, unsafe {
            oak_abi::channel_close(out_channel)
        });
        expect_eq!(OakStatus::Ok as u32, unsafe {
            oak_abi::channel_close(in_channel)
        });
        Ok(())
    }

    fn test_channel_read(&mut self) -> TestResult {
        let (out_channel, in_channel) = oak::channel_create().unwrap();

        // No message pending.
        let mut buffer = Vec::<u8>::with_capacity(5);
        let mut handles = Vec::with_capacity(1);
        expect_eq!(
            Err(OakStatus::ErrChannelEmpty),
            oak::channel_read(in_channel, &mut buffer, &mut handles)
        );
        expect_eq!(0, buffer.len());
        expect_eq!(0, handles.len());

        // No message pending clears any data in input vectors.
        let mut nonempty_buffer = vec![0x91, 0x92, 0x93];
        let mut nonempty_handles = vec![out_channel.handle];
        expect_eq!(
            Err(OakStatus::ErrChannelEmpty),
            oak::channel_read(in_channel, &mut nonempty_buffer, &mut nonempty_handles)
        );
        expect_eq!(0, nonempty_buffer.len());
        expect_eq!(0, nonempty_handles.len());

        // Single message.
        let data = vec![0x01, 0x02, 0x03];
        expect_eq!(Ok(()), oak::channel_write(out_channel, &data, &[]));
        expect_eq!(
            Ok(()),
            oak::channel_read(in_channel, &mut buffer, &mut handles)
        );
        expect_eq!(3, buffer.len());
        expect_eq!(0, handles.len());

        // Reading again zeroes the vector length.
        expect_eq!(
            Err(OakStatus::ErrChannelEmpty),
            oak::channel_read(in_channel, &mut buffer, &mut handles)
        );
        expect_eq!(0, buffer.len());
        expect_eq!(0, handles.len());
        expect_eq!(5, buffer.capacity());

        // Now send and receive a bigger message.
        let data = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        expect_eq!(Ok(()), oak::channel_write(out_channel, &data, &[]));
        expect_eq!(
            Ok(()),
            oak::channel_read(in_channel, &mut buffer, &mut handles)
        );
        expect_eq!(8, buffer.len());
        expect_eq!(0, handles.len());
        expect!(buffer.capacity() >= 8);

        // Reading from an invalid handle gives an error.
        let bogus_channel = oak::ReadHandle {
            handle: oak::Handle::from_raw(99999),
        };
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::channel_read(bogus_channel, &mut buffer, &mut handles)
        );

        // Send and receive lots of handles.
        let data = vec![0x01, 0x02, 0x03];
        expect_eq!(
            Ok(()),
            oak::channel_write(
                out_channel,
                &data,
                &[
                    out_channel.handle,
                    out_channel.handle,
                    out_channel.handle,
                    out_channel.handle
                ]
            )
        );
        expect_eq!(
            Ok(()),
            oak::channel_read(in_channel, &mut buffer, &mut handles)
        );
        expect_eq!(3, buffer.len());
        expect_eq!(4, handles.len());
        for handle in &handles {
            oak::channel_close(*handle).expect("could not close channel");
        }

        // Reading again clears the buffer and the handles.
        expect_eq!(
            Err(OakStatus::ErrChannelEmpty),
            oak::channel_read(in_channel, &mut buffer, &mut handles)
        );
        expect_eq!(0, buffer.len());
        expect_eq!(0, handles.len());

        expect_eq!(Ok(()), oak::channel_close(out_channel.handle));
        expect_eq!(Ok(()), oak::channel_close(in_channel.handle));
        Ok(())
    }

    fn test_channel_read_orphan(&mut self) -> TestResult {
        let (out_channel, in_channel) = oak::channel_create().unwrap();

        // Drop the only write handle for this channel.
        expect_eq!(Ok(()), oak::channel_close(out_channel.handle));

        // An attempt to read now fails.
        let mut buffer = Vec::<u8>::with_capacity(5);
        let mut handles = Vec::with_capacity(5);
        expect_eq!(
            Err(OakStatus::ErrChannelClosed),
            oak::channel_read(in_channel, &mut buffer, &mut handles)
        );

        expect_eq!(Ok(()), oak::channel_close(in_channel.handle));
        Ok(())
    }

    fn test_channel_write_raw(&mut self) -> TestResult {
        let (out_channel, in_channel, _) = channel_create_raw();

        let buf = vec![0x01];
        let handles = vec![in_channel];
        unsafe {
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_write(
                    out_channel,
                    invalid_raw_offset() as *const u8,
                    1,
                    handles.as_ptr() as *const u8,
                    handles.len() as u32,
                )
            );
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::channel_write(
                    out_channel,
                    buf.as_ptr(),
                    buf.len(),
                    invalid_raw_offset() as *const u8,
                    1,
                )
            );
        }
        expect_eq!(OakStatus::Ok as u32, unsafe {
            oak_abi::channel_close(in_channel)
        });
        expect_eq!(OakStatus::Ok as u32, unsafe {
            oak_abi::channel_close(out_channel)
        });
        Ok(())
    }

    fn test_channel_write(&mut self) -> TestResult {
        let (out_channel, in_channel) = oak::channel_create().unwrap();

        // Empty message.
        let empty = vec![];
        expect_eq!(Ok(()), oak::channel_write(out_channel, &empty, &[]));

        // Single message.
        let data = vec![0x01, 0x02, 0x03];
        expect_eq!(Ok(()), oak::channel_write(out_channel, &data, &[]));

        // Now send a bigger message.
        let data = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        expect_eq!(Ok(()), oak::channel_write(out_channel, &data, &[]));

        // Writing to an invalid handle gives an error.
        let bogus_channel = oak::WriteHandle {
            handle: oak::Handle::from_raw(99999),
        };
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::channel_write(bogus_channel, &data, &[])
        );

        expect_eq!(Ok(()), oak::channel_close(in_channel.handle));
        expect_eq!(Ok(()), oak::channel_close(out_channel.handle));
        Ok(())
    }

    fn test_channel_write_orphan(&mut self) -> TestResult {
        let (out_channel, in_channel) = oak::channel_create().unwrap();

        // Close the only read handle for the channel.
        expect_eq!(Ok(()), oak::channel_close(in_channel.handle));

        // There's no way to read from the channel, so writing fails.
        let data = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        expect_eq!(
            Err(OakStatus::ErrChannelClosed),
            oak::channel_write(out_channel, &data, &[])
        );

        expect_eq!(Ok(()), oak::channel_close(out_channel.handle));
        Ok(())
    }

    fn test_channel_wait_raw(&mut self) -> TestResult {
        let (out_channel, in_channel, _) = channel_create_raw();
        let (out_empty_channel, in_empty_channel, _) = channel_create_raw();

        unsafe {
            // Write a message to the channel so wait operations don't block.
            let data = vec![0x01, 0x02, 0x03];
            expect_eq!(
                OakStatus::Ok as u32,
                oak_abi::channel_write(out_channel, data.as_ptr(), data.len(), &[] as *const u8, 0)
            );

            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::wait_on_channels(invalid_raw_offset() as *mut u8, 1)
            );
        }

        unsafe {
            // Wait on [write handle, ready read handle, not-ready read handle].
            const COUNT: usize = 3;
            let mut space = Vec::with_capacity(COUNT * oak_abi::SPACE_BYTES_PER_HANDLE);
            space
                .write_u64::<byteorder::LittleEndian>(out_channel)
                .unwrap();
            space.push(0x00);
            space
                .write_u64::<byteorder::LittleEndian>(in_channel)
                .unwrap();
            space.push(0x00);
            space
                .write_u64::<byteorder::LittleEndian>(in_empty_channel)
                .unwrap();
            space.push(0x00);

            expect_eq!(
                OakStatus::Ok as u32,
                oak_abi::wait_on_channels(space.as_mut_ptr(), COUNT as u32)
            );
            expect_eq!(
                ChannelReadStatus::InvalidChannel as i32,
                i32::from(space[8])
            );
            expect_eq!(ChannelReadStatus::ReadReady as i32, i32::from(space[9 + 8]));
            expect_eq!(
                ChannelReadStatus::NotReady as i32,
                i32::from(space[9 + 9 + 8])
            );
        }

        unsafe {
            // Wait on [nonsense handle, read handle].
            const COUNT: usize = 2;
            let mut space = Vec::with_capacity(COUNT * oak_abi::SPACE_BYTES_PER_HANDLE);
            space
                .write_u64::<byteorder::LittleEndian>(9_123_456)
                .unwrap();
            space.push(0x00);
            space
                .write_u64::<byteorder::LittleEndian>(in_channel)
                .unwrap();
            space.push(0x00);
            expect_eq!(
                OakStatus::Ok as u32,
                oak_abi::wait_on_channels(space.as_mut_ptr(), COUNT as u32)
            );
            expect_eq!(
                ChannelReadStatus::InvalidChannel as i32,
                i32::from(space[8])
            );
            expect_eq!(ChannelReadStatus::ReadReady as i32, i32::from(space[9 + 8]));
            expect_eq!(OakStatus::Ok as u32, oak_abi::channel_close(out_channel));
        }

        // Still a pending message on in_channel even though the only write half for
        // the channel is closed.
        unsafe {
            const COUNT: usize = 1;
            let mut space = Vec::with_capacity(COUNT * oak_abi::SPACE_BYTES_PER_HANDLE);
            space
                .write_u64::<byteorder::LittleEndian>(in_channel)
                .unwrap();
            space.push(0x00);
            expect_eq!(
                OakStatus::Ok as u32,
                oak_abi::wait_on_channels(space.as_mut_ptr(), COUNT as u32)
            );
            expect_eq!(ChannelReadStatus::ReadReady as i32, i32::from(space[8]));
        }
        // Consume the pending message.
        let mut buffer = Vec::with_capacity(5);
        let mut recv_len = 0u32;
        let mut handles = Vec::with_capacity(5);
        let mut recv_handles = 0u32;
        unsafe {
            expect_eq!(
                OakStatus::Ok as u32,
                oak_abi::channel_read(
                    in_channel,
                    buffer.as_mut_ptr() as *mut u8,
                    buffer.capacity(),
                    &mut recv_len,
                    handles.as_mut_ptr() as *mut u8,
                    handles.capacity() as u32,
                    &mut recv_handles
                )
            );
        }
        expect_eq!(3, recv_len);
        expect_eq!(0, recv_handles);

        // Read half is now orphaned (no pending message, no possible writers).
        unsafe {
            const COUNT: usize = 1;
            let mut space = Vec::with_capacity(COUNT * oak_abi::SPACE_BYTES_PER_HANDLE);
            space
                .write_u64::<byteorder::LittleEndian>(in_channel)
                .unwrap();
            space.push(0x00);
            expect_eq!(
                OakStatus::ErrBadHandle as u32,
                oak_abi::wait_on_channels(space.as_mut_ptr(), COUNT as u32)
            );
            expect_eq!(ChannelReadStatus::Orphaned as i32, i32::from(space[8]));

            expect_eq!(OakStatus::Ok as u32, oak_abi::channel_close(in_channel));
        }

        unsafe {
            expect_eq!(
                OakStatus::Ok as u32,
                oak_abi::channel_close(in_empty_channel)
            );
            expect_eq!(
                OakStatus::Ok as u32,
                oak_abi::channel_close(out_empty_channel)
            );
        }

        Ok(())
    }

    fn test_channel_wait(&mut self) -> TestResult {
        let (out1, in1) = oak::channel_create().unwrap();
        let (out2, in2) = oak::channel_create().unwrap();

        // Waiting on (just) non-read channel handles should fail immediately.
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::wait_on_channels(&[
                oak::ReadHandle {
                    handle: out1.handle
                },
                oak::ReadHandle {
                    handle: oak::Handle::from_raw(9_999_999)
                }
            ])
        );

        // Set up first channel with a pending message.
        let data = vec![0x01, 0x02, 0x03];
        expect_eq!(Ok(()), oak::channel_write(out1, &data, &[]));

        expect_eq!(
            vec![ChannelReadStatus::ReadReady, ChannelReadStatus::NotReady],
            status_convert(oak::wait_on_channels(&[in1, in2]))?
        );
        // No read so still ready (level triggered not edge triggered).
        expect_eq!(
            vec![ChannelReadStatus::ReadReady, ChannelReadStatus::NotReady],
            status_convert(oak::wait_on_channels(&[in1, in2]))?
        );

        expect_eq!(Ok(()), oak::channel_write(out2, &data, &[]));
        expect_eq!(
            vec![ChannelReadStatus::ReadReady, ChannelReadStatus::ReadReady],
            status_convert(oak::wait_on_channels(&[in1, in2]))?
        );

        let mut buffer = Vec::<u8>::with_capacity(5);
        let mut handles = Vec::with_capacity(5);
        expect_eq!(Ok(()), oak::channel_read(in1, &mut buffer, &mut handles));
        expect_eq!(3, buffer.len());
        expect_eq!(0, handles.len());

        expect_eq!(
            vec![ChannelReadStatus::NotReady, ChannelReadStatus::ReadReady],
            status_convert(oak::wait_on_channels(&[in1, in2]))?
        );

        // Write channels and nonsense handles are ignored.
        expect_eq!(
            vec![
                ChannelReadStatus::NotReady,
                ChannelReadStatus::ReadReady,
                ChannelReadStatus::InvalidChannel
            ],
            status_convert(oak::wait_on_channels(&[
                in1,
                in2,
                oak::ReadHandle {
                    handle: out1.handle
                }
            ]))?
        );
        expect_eq!(
            vec![
                ChannelReadStatus::NotReady,
                ChannelReadStatus::ReadReady,
                ChannelReadStatus::InvalidChannel
            ],
            status_convert(oak::wait_on_channels(&[
                in1,
                in2,
                oak::ReadHandle {
                    handle: oak::Handle::from_raw(9_999_999)
                }
            ]))?
        );

        expect_eq!(Ok(()), oak::channel_close(out1.handle));
        expect_eq!(Ok(()), oak::channel_close(out2.handle));

        // Still a pending message on in2 even though the only write half for
        // the channel is closed.
        expect_eq!(
            vec![ChannelReadStatus::ReadReady],
            status_convert(oak::wait_on_channels(&[in2]))?
        );

        expect_eq!(Ok(()), oak::channel_close(in1.handle));
        expect_eq!(Ok(()), oak::channel_close(in2.handle));
        Ok(())
    }

    fn test_channel_wait_orphan(&mut self) -> TestResult {
        // Use 2 channels so there's always a ready channel to prevent
        // wait_on_channels blocking.
        let (out1, in1) = oak::channel_create().unwrap();
        let (out2, in2) = oak::channel_create().unwrap();

        // Set up pending messages so each channel is read-ready.
        let data = vec![0x01, 0x02, 0x03];
        expect_eq!(Ok(()), oak::channel_write(out1, &data, &[]));
        expect_eq!(Ok(()), oak::channel_write(out2, &data, &[]));
        expect_eq!(
            vec![ChannelReadStatus::ReadReady, ChannelReadStatus::ReadReady],
            status_convert(oak::wait_on_channels(&[in1, in2]))?
        );

        // Close the only write handle to channel 1.
        expect_eq!(Ok(()), oak::channel_close(out1.handle));

        // Channel is still read-ready because there's a queued message.
        expect_eq!(
            vec![ChannelReadStatus::ReadReady],
            status_convert(oak::wait_on_channels(&[in1]))?
        );

        // Consume the only message on channel 1.
        let mut buffer = Vec::<u8>::with_capacity(5);
        let mut handles = Vec::with_capacity(5);
        expect_eq!(Ok(()), oak::channel_read(in1, &mut buffer, &mut handles));
        expect_eq!(3, buffer.len());
        expect_eq!(0, handles.len());

        // Now expect the channel status to be orphaned.
        expect_eq!(
            vec![ChannelReadStatus::Orphaned, ChannelReadStatus::ReadReady],
            status_convert(oak::wait_on_channels(&[in1, in2]))?
        );

        expect_eq!(Ok(()), oak::channel_close(in1.handle));
        expect_eq!(Ok(()), oak::channel_close(in2.handle));
        expect_eq!(Ok(()), oak::channel_close(out2.handle));
        Ok(())
    }

    fn test_node_create_raw(&mut self) -> TestResult {
        let (_, in_channel, _) = channel_create_raw();

        let valid = "a_string";
        let non_utf8_name: Vec<u8> = vec![0xc3, 0x28];
        unsafe {
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::node_create(
                    invalid_raw_offset() as *mut u8,
                    1,
                    valid.as_ptr(),
                    valid.len(),
                    in_channel
                )
            );

            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::node_create(
                    non_utf8_name.as_ptr(),
                    non_utf8_name.len(),
                    valid.as_ptr(),
                    valid.len(),
                    in_channel
                )
            );

            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::node_create(
                    valid.as_ptr(),
                    valid.len(),
                    invalid_raw_offset() as *mut u8,
                    1,
                    in_channel
                )
            );

            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::node_create(
                    valid.as_ptr(),
                    valid.len(),
                    non_utf8_name.as_ptr(),
                    non_utf8_name.len(),
                    in_channel
                )
            );
        }
        Ok(())
    }
    fn test_node_create(&mut self) -> TestResult {
        expect_eq!(
            Err(OakStatus::ErrInvalidArgs),
            oak::node_create(
                "no-such-config",
                BACKEND_ENTRYPOINT_NAME,
                self.backend_in[0]
            )
        );
        expect_eq!(
            Err(OakStatus::ErrInvalidArgs),
            oak::node_create(
                BACKEND_CONFIG_NAME,
                "no-such-entrypoint",
                self.backend_in[0]
            )
        );
        expect_eq!(
            Err(OakStatus::ErrInvalidArgs),
            oak::node_create(
                BACKEND_CONFIG_NAME,
                "backend_fake_main", // exists but wrong signature
                self.backend_in[0]
            )
        );
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::node_create(
                BACKEND_CONFIG_NAME,
                BACKEND_ENTRYPOINT_NAME,
                oak::ReadHandle {
                    handle: oak::Handle::from_raw(oak_abi::INVALID_HANDLE)
                }
            )
        );
        let (out_handle, in_handle) = oak::channel_create().unwrap();
        expect_eq!(
            Ok(()),
            oak::node_create(BACKEND_CONFIG_NAME, BACKEND_ENTRYPOINT_NAME, in_handle)
        );
        expect_eq!(
            Ok(()),
            oak::node_create(BACKEND_CONFIG_NAME, BACKEND_ENTRYPOINT_NAME, in_handle)
        );

        expect_eq!(Ok(()), oak::channel_close(in_handle.handle));
        expect_eq!(Ok(()), oak::channel_close(out_handle.handle));
        Ok(())
    }

    fn test_random_get_raw(&mut self) -> TestResult {
        unsafe {
            expect_eq!(
                OakStatus::ErrInvalidArgs as u32,
                oak_abi::random_get(invalid_raw_offset() as *mut u8, 1)
            );
        }
        Ok(())
    }

    fn test_random_get(&mut self) -> TestResult {
        let original = vec![0x01, 0x02, 0x03, 0x04];
        let mut data = original.clone();
        expect_eq!(Ok(()), oak::random_get(&mut data));
        // 1 in 2^32 chance of getting back original value
        expect!(data != original);
        Ok(())
    }

    fn test_random_rng(&mut self) -> TestResult {
        let mut rng = oak::rand::OakRng {};
        let x1 = rng.gen::<u64>();
        let x2 = rng.gen::<u64>();
        expect!(x1 != x2);
        Ok(())
    }

    fn test_channel_handle_reuse(&mut self) -> TestResult {
        // Set up a fresh channel with a pending message so wait_on_channels
        // doesn't block.
        let (out_handle, in_handle) = oak::channel_create().unwrap();
        let data = vec![0x01, 0x02, 0x03];
        expect_eq!(Ok(()), oak::channel_write(out_handle, &data, &[]));

        // Read from an invalid handle.
        let mut buffer = Vec::<u8>::with_capacity(5);
        let mut handles = Vec::with_capacity(5);
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::channel_read(
                oak::ReadHandle {
                    handle: oak::Handle::from_raw(9_987_123)
                },
                &mut buffer,
                &mut handles
            )
        );
        // Wait on an invalid handle.
        expect_eq!(
            Ok(vec![
                ChannelReadStatus::ReadReady,
                ChannelReadStatus::InvalidChannel
            ]),
            oak::wait_on_channels(&[
                in_handle,
                oak::ReadHandle {
                    handle: oak::Handle::from_raw(9_987_321)
                }
            ])
        );

        // Close both of the previously mentioned invalid handles.
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::channel_close(oak::Handle::from_raw(9_987_123))
        );
        expect_eq!(
            Err(OakStatus::ErrBadHandle),
            oak::channel_close(oak::Handle::from_raw(9_987_321))
        );

        expect_eq!(Ok(()), oak::channel_close(out_handle.handle));
        expect_eq!(Ok(()), oak::channel_close(in_handle.handle));
        Ok(())
    }

    fn test_direct_log(&mut self) -> TestResult {
        // Send a message directly to a fresh logging Node (not via the log facade).
        // Include some handles which will be ignored.
        let (logging_handle, read_handle) =
            oak::channel_create().expect("could not create channel");
        oak::node_create(LOG_CONFIG_NAME, "oak_main", read_handle).expect("could not create node");
        oak::channel_close(read_handle.handle).expect("could not close channel");

        expect!(logging_handle.handle.is_valid());
        let (out_handle, in_handle) = oak::channel_create().expect("could not create channel");

        oak::channel_write(
            logging_handle,
            "Malformed message sent direct to logging channel!".as_bytes(),
            &[in_handle.handle, out_handle.handle],
        )
        .expect("could not write to channel");

        oak::channel_write(
            logging_handle,
            &oak::proto::log::LogMessage {
                level: oak::proto::log::Level::INFO,
                file: "abitest".to_string(),
                message: "Wellformed message sent direct to logging channel!".to_string(),
                ..Default::default()
            }
            .write_to_bytes()
            .unwrap()[..],
            &[in_handle.handle, out_handle.handle],
        )
        .expect("could not write to channel");

        expect_eq!(Ok(()), oak::channel_close(out_handle.handle));
        expect_eq!(Ok(()), oak::channel_close(in_handle.handle));
        expect_eq!(Ok(()), oak::channel_close(logging_handle.handle));
        Ok(())
    }

    fn test_backend_roundtrip(&mut self) -> TestResult {
        // Make a collection of new channels for the backend nodes to read from,
        // and send the read handles to each backend node.
        const CHANNEL_COUNT: usize = 3;
        let mut read_handles = Vec::with_capacity(CHANNEL_COUNT);
        let mut write_handles = Vec::with_capacity(CHANNEL_COUNT);
        for _ in 0..CHANNEL_COUNT {
            let (new_write, new_read) = oak::channel_create().unwrap();
            write_handles.push(new_write);
            read_handles.push(new_read.handle);
        }
        for j in 0..BACKEND_COUNT {
            expect_eq!(
                Ok(()),
                oak::channel_write(self.backend_out[j], &[], &read_handles)
            );
        }
        for new_read in read_handles.iter() {
            oak::channel_close(*new_read).expect("could not close channel");
        }

        // Ask the backend node to transmute something by writing a serialized
        // request to one of the new channels that the backends just received
        // the read handles for.
        for new_write in write_handles.iter() {
            let internal_req = InternalMessage {
                msg: "aaa".to_string(),
            };
            info!(
                "sending message to new channel {:?}: {:?}",
                new_write.handle, internal_req,
            );
            let new_channel = oak::io::Sender::new(*new_write);
            new_channel
                .send(&internal_req)
                .expect("could not send request over channel");
            oak::channel_close(new_write.handle).expect("could not close channel");

            // Block until there is a response from one of the backends
            // available.
            let readies = oak::wait_on_channels(&self.backend_in).map_err(OakError::OakStatus)?;

            // Expect exactly one of the backends to have received
            // the message.
            let mut buffer = Vec::<u8>::with_capacity(256);
            let mut new_in_channel = oak::ReadHandle {
                handle: oak::Handle::from_raw(0),
            };
            for (j, ready) in readies.iter().enumerate() {
                if *ready == ChannelReadStatus::ReadReady {
                    info!("got response from backend[{}]", j);
                    expect_eq!(oak::Handle::from_raw(0), new_in_channel.handle);
                    let mut handles = Vec::with_capacity(1);
                    expect_eq!(
                        Ok(()),
                        oak::channel_read(self.backend_in[j], &mut buffer, &mut handles)
                    );
                    new_in_channel.handle = handles[0];
                }
            }

            // Expect the response to hold a channel read handle.
            // Read the actual transmuted message from this new channel.
            expect_eq!(
                Ok(()),
                oak::channel_read(new_in_channel, &mut buffer, &mut vec![])
            );
            let serialized_rsp = String::from_utf8(buffer).unwrap();
            let internal_rsp: InternalMessage = serde_json::from_str(&serialized_rsp)?;
            expect_eq!("aaaxxx", internal_rsp.msg);

            // Drop the new read channel now we have got the response.
            expect_eq!(Ok(()), oak::channel_close(new_in_channel.handle));
        }

        // Send a single byte to each backend Node, so they get woken up to detect
        // orphaned channels.
        for j in 0..BACKEND_COUNT {
            expect_eq!(
                Ok(()),
                oak::channel_write(self.backend_out[j], &[0x00], &[])
            );
        }
        Ok(())
    }
    fn test_storage(&mut self) -> TestResult {
        let storage = self.storage.as_mut().ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no storage channel available",
            ))
        })?;
        let key = b"test-key-0";
        let value = b"test-value-0";
        storage
            .write(&self.storage_name, key, value)
            .map_err(from_proto)?;

        let got = storage.read(&self.storage_name, key).map_err(from_proto)?;
        expect_eq!(value.to_vec(), got);

        let key2 = b"test-key-bogus";
        let got = storage.read(&self.storage_name, key2);
        expect_matches!(got, Err(_));
        expect_eq!(grpc::Code::NOT_FOUND.value(), got.unwrap_err().get_code());

        let got = storage.read(&self.storage_name, key).map_err(from_proto)?;
        expect_eq!(value.to_vec(), got);

        storage
            .delete(&self.storage_name, key)
            .map_err(from_proto)?;

        let got = storage.read(&self.storage_name, key);
        expect_matches!(got, Err(_));
        expect_eq!(grpc::Code::NOT_FOUND.value(), got.unwrap_err().get_code());

        storage
            .delete(&self.storage_name, key)
            .map_err(from_proto)?;
        storage
            .delete(&self.storage_name, key2)
            .map_err(from_proto)?;

        Ok(())
    }
    fn test_absent_storage(&mut self) -> TestResult {
        // Expect to have a channel to a storage pseudo-Node, but the remote
        // storage provider is unavailable.
        let storage = self.absent_storage.as_mut().ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no storage channel available",
            ))
        })?;
        let key = b"a-test-key-0";
        let value = b"a-test-value-0";

        let got = storage.write(&self.storage_name, key, value);
        expect_matches!(got, Err(_));
        expect_eq!(grpc::Code::UNAVAILABLE.value(), got.unwrap_err().get_code());
        let got = storage.read(&self.storage_name, key);
        expect_matches!(got, Err(_));
        expect_eq!(grpc::Code::UNAVAILABLE.value(), got.unwrap_err().get_code());
        let got = storage.delete(&self.storage_name, key);
        expect_matches!(got, Err(_));
        expect_eq!(grpc::Code::UNAVAILABLE.value(), got.unwrap_err().get_code());

        Ok(())
    }
    fn test_grpc_client_unary_method(&mut self) -> TestResult {
        let grpc_stub = self.grpc_service.as_ref().ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no gRPC client channel available",
            ))
        })?;

        // Successful unary method invocation of external service via gRPC client pseudo-Node.
        let ok_req = GrpcTestRequest {
            method_result: Some(ok_text("test".to_string())),
            ..Default::default()
        };
        let ok_rsp = GrpcTestResponse {
            text: "test".to_string(),
            ..Default::default()
        };
        expect_eq!(grpc_stub.unary_method(ok_req), Ok(ok_rsp));

        // Errored unary method invocation of external service via gRPC client pseudo-Node.
        let err_req = GrpcTestRequest {
            method_result: Some(err_code(grpc::Code::FAILED_PRECONDITION.value())),
            ..Default::default()
        };
        let result = grpc_stub.unary_method(err_req);
        expect_matches!(result, Err(_));
        expect_eq!(
            grpc::Code::FAILED_PRECONDITION.value(),
            result.unwrap_err().get_code()
        );

        Ok(())
    }
    fn test_grpc_client_server_streaming_method(&mut self) -> TestResult {
        let grpc_stub = self.grpc_service.as_ref().ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no gRPC client channel available",
            ))
        })?;
        // Successful server-streaming method invocation of external service via
        // gRPC client pseudo-Node.
        let ok_req = GrpcTestRequest {
            method_result: Some(ok_text("test".to_string())),
            ..Default::default()
        };
        let ok_rsp = GrpcTestResponse {
            text: "test".to_string(),
            ..Default::default()
        };
        let receiver = grpc_stub
            .server_streaming_method(ok_req)
            .map_err(from_proto)?;
        let mut count = 0;
        loop {
            match receiver.receive() {
                Ok(grpc_rsp) => {
                    // TODO(#592): get typed response directly from receiver.
                    expect_eq!(oak::grpc::Code::OK.value(), grpc_rsp.get_status().code);
                    let rsp: GrpcTestResponse =
                        protobuf::parse_from_bytes(&grpc_rsp.get_rsp_msg().value)
                            .expect("failed to parse GrpcTestResponse");
                    expect_eq!(rsp.clone(), ok_rsp.clone());
                    count += 1;
                }
                Err(OakError::OakStatus(OakStatus::ErrBadHandle)) => break,
                Err(OakError::OakStatus(OakStatus::ErrChannelClosed)) => break,
                Err(e) => return Err(Box::new(e)),
            }
        }
        expect_eq!(2, count);
        receiver.close().expect("failed to close receiver");

        // Errored server-streaming method invocation of external service via
        // gRPC client pseudo-Node.
        let err_req = GrpcTestRequest {
            method_result: Some(err_code(grpc::Code::FAILED_PRECONDITION.value())),
            ..Default::default()
        };
        let receiver = grpc_stub
            .server_streaming_method(err_req)
            .map_err(from_proto)?;
        let mut seen_err_code = false;
        loop {
            match receiver.receive() {
                Ok(grpc_rsp) => {
                    expect_eq!(
                        grpc::Code::FAILED_PRECONDITION.value(),
                        grpc_rsp.get_status().code
                    );
                    seen_err_code = true;
                }
                Err(OakError::OakStatus(OakStatus::ErrBadHandle)) => break,
                Err(OakError::OakStatus(OakStatus::ErrChannelClosed)) => break,
                Err(e) => return Err(Box::new(e)),
            }
        }
        expect!(seen_err_code);
        receiver.close().expect("failed to close receiver");
        Ok(())
    }
    fn test_absent_grpc_client(&mut self) -> TestResult {
        // Expect to have a channel to a gRPC client pseudo-Node, but the remote
        // gRPC service is unavailable.
        let grpc_stub = self.absent_grpc_service.as_ref().ok_or_else(|| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no gRPC client channel available",
            ))
        })?;
        let req = GrpcTestRequest {
            method_result: Some(ok_text("test".to_string())),
            ..Default::default()
        };

        // Attempting to invoke any sort of method should fail.
        let result = grpc_stub.unary_method(req.clone());
        info!("absent.unary_method() -> {:?}", result);
        expect_matches!(result, Err(_));

        info!("absent.server_streaming_method()");
        let receiver = grpc_stub.server_streaming_method(req).map_err(from_proto)?;
        loop {
            let result = receiver.receive();
            info!("absent.server_streaming_method().receive() -> {:?}", result);
            match result {
                Ok(grpc_rsp) => {
                    expect!(grpc_rsp.get_status().code != grpc::Code::OK.value());
                }
                Err(OakError::OakStatus(OakStatus::ErrBadHandle)) => break,
                Err(OakError::OakStatus(OakStatus::ErrChannelClosed)) => break,
                Err(e) => return Err(Box::new(e)),
            }
        }
        receiver.close().expect("failed to close receiver");

        Ok(())
    }
}

// Helper for storage error conversion.
fn from_proto(status: oak::proto::status::Status) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("status code {} message '{}'", status.code, status.message),
    ))
}
