/*
 * Copyright 2019 The Project Oak Authors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#include "oak/server/storage/storage_node.h"

#include "absl/memory/memory.h"
#include "absl/strings/escaping.h"
#include "grpcpp/create_channel.h"
#include "oak/common/logging.h"
#include "oak/proto/grpc_encap.pb.h"
#include "oak/proto/storage_channel.pb.h"
#include "oak/server/invocation.h"
#include "third_party/asylo/status_macros.h"

namespace oak {

StorageNode::StorageNode(BaseRuntime* runtime, const std::string& name,
                         const std::string& storage_address)
    : NodeThread(runtime, name), storage_processor_(storage_address) {}

void StorageNode::Run(Handle invocation_handle) {
  // Borrow a pointer to the relevant channel half.
  MessageChannelReadHalf* invocation_channel = BorrowReadChannel(invocation_handle);
  if (invocation_channel == nullptr) {
    OAK_LOG(ERROR) << "Required channel not available; handle: " << invocation_handle;
    return;
  }
  std::vector<std::unique_ptr<ChannelStatus>> channel_status;
  channel_status.push_back(absl::make_unique<ChannelStatus>(invocation_handle));
  while (true) {
    if (!WaitOnChannels(&channel_status)) {
      OAK_LOG(WARNING) << "Node termination requested";
      return;
    }

    std::unique_ptr<Invocation> invocation(Invocation::ReceiveFromChannel(invocation_channel));
    if (invocation == nullptr) {
      OAK_LOG(ERROR) << "Failed to create invocation";
      return;
    }

    // Expect to read a single request out of the request channel.
    ReadResult req_result = invocation->req_channel->Read(INT_MAX, INT_MAX);
    if (req_result.required_size > 0) {
      OAK_LOG(ERROR) << "Message size too large: " << req_result.required_size;
      return;
    }
    if (req_result.msg->channels.size() != 0) {
      OAK_LOG(ERROR) << "Unexpectedly received channel handles in request channel";
      return;
    }
    GrpcRequest grpc_req;
    grpc_req.ParseFromString(std::string(req_result.msg->data.data(), req_result.msg->data.size()));

    std::unique_ptr<GrpcResponse> grpc_rsp;
    oak::StatusOr<std::unique_ptr<GrpcResponse>> rsp_or = ProcessMethod(&grpc_req);
    if (!rsp_or.ok()) {
      OAK_LOG(ERROR) << "Failed to perform " << grpc_req.method_name() << ": "
                     << rsp_or.status().code() << ", '" << rsp_or.status().message() << "'";
      grpc_rsp = absl::make_unique<GrpcResponse>();
      grpc_rsp->mutable_status()->set_code(static_cast<int>(rsp_or.status().code()));
      grpc_rsp->mutable_status()->set_message(std::string(rsp_or.status().message()));
    } else {
      grpc_rsp = std::move(rsp_or).ValueOrDie();
    }

    grpc_rsp->set_last(true);
    std::unique_ptr<Message> rsp_msg = absl::make_unique<Message>();
    size_t serialized_size = grpc_rsp->ByteSizeLong();
    rsp_msg->data.resize(serialized_size);
    grpc_rsp->SerializeToArray(rsp_msg->data.data(), rsp_msg->data.size());
    invocation->rsp_channel->Write(std::move(rsp_msg));

    // The response channel reference is dropped here.
  }
}

oak::StatusOr<std::unique_ptr<GrpcResponse>> StorageNode::ProcessMethod(GrpcRequest* grpc_req) {
  auto grpc_rsp = absl::make_unique<GrpcResponse>();
  grpc::Status status;
  std::string method_name = grpc_req->method_name();

  if (method_name == "/oak.StorageNode/Read") {
    StorageChannelReadRequest read_req;
    if (!grpc_req->req_msg().UnpackTo(&read_req)) {
      return absl::Status(absl::StatusCode::kInvalidArgument, "Failed to unpack request");
    }
    StorageChannelReadResponse read_rsp;
    std::string value;
    OAK_ASSIGN_OR_RETURN(value,
                         storage_processor_.Read(read_req.storage_name(), read_req.item().name(),
                                                 read_req.transaction_id()));
    read_rsp.mutable_item()->ParseFromString(value);
    // TODO(#449): Check security policy for item.
    grpc_rsp->mutable_rsp_msg()->PackFrom(read_rsp);

  } else if (method_name == "/oak.StorageNode/Write") {
    StorageChannelWriteRequest write_req;
    if (!grpc_req->req_msg().UnpackTo(&write_req)) {
      return absl::Status(absl::StatusCode::kInvalidArgument, "Failed to unpack request");
    }
    // TODO(#449): Check integrity policy for item.
    std::string item;
    write_req.item().SerializeToString(&item);
    OAK_RETURN_IF_ERROR(storage_processor_.Write(write_req.storage_name(), write_req.item().name(),
                                                 item, write_req.transaction_id()));

  } else if (method_name == "/oak.StorageNode/Delete") {
    StorageChannelDeleteRequest delete_req;
    if (!grpc_req->req_msg().UnpackTo(&delete_req)) {
      return absl::Status(absl::StatusCode::kInvalidArgument, "Failed to unpack request");
    }
    // TODO(#449): Check integrity policy for item.
    OAK_RETURN_IF_ERROR(storage_processor_.Delete(
        delete_req.storage_name(), delete_req.item().name(), delete_req.transaction_id()));

  } else if (method_name == "/oak.StorageNode/Begin") {
    StorageChannelBeginRequest begin_req;
    if (!grpc_req->req_msg().UnpackTo(&begin_req)) {
      return absl::Status(absl::StatusCode::kInvalidArgument, "Failed to unpack request");
    }
    StorageChannelBeginResponse begin_rsp;
    std::string transaction_id;
    OAK_ASSIGN_OR_RETURN(transaction_id, storage_processor_.Begin(begin_req.storage_name()));
    begin_rsp.set_transaction_id(transaction_id);
    grpc_rsp->mutable_rsp_msg()->PackFrom(begin_rsp);

  } else if (method_name == "/oak.StorageNode/Commit") {
    StorageChannelCommitRequest commit_req;
    if (!grpc_req->req_msg().UnpackTo(&commit_req)) {
      return absl::Status(absl::StatusCode::kInvalidArgument, "Failed to unpack request");
    }
    OAK_RETURN_IF_ERROR(
        storage_processor_.Commit(commit_req.storage_name(), commit_req.transaction_id()));

  } else if (method_name == "/oak.StorageNode/Rollback") {
    StorageChannelRollbackRequest rollback_req;
    if (!grpc_req->req_msg().UnpackTo(&rollback_req)) {
      return absl::Status(absl::StatusCode::kInvalidArgument, "Failed to unpack request");
    }
    OAK_RETURN_IF_ERROR(
        storage_processor_.Rollback(rollback_req.storage_name(), rollback_req.transaction_id()));
  } else {
    OAK_LOG(ERROR) << "unknown operation " << method_name;
    return absl::Status(absl::StatusCode::kInvalidArgument, "Unknown operation request method.");
  }
  return std::move(grpc_rsp);
}

}  // namespace oak
