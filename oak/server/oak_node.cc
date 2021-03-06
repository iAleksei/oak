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

#include "oak/server/oak_node.h"

#include "oak/common/logging.h"
#include "oak/server/notification.h"

namespace oak {

Handle OakNode::NextHandle() {
  std::uniform_int_distribution<Handle> distribution;
  while (true) {
    // Keep picking random Handle values until we find an unused (and valid) value.
    Handle handle = distribution(prng_engine_);
    if (handle == kInvalidHandle) {
      continue;
    }
    if (channel_halves_.find(handle) == channel_halves_.end()) {
      return handle;
    }
  }
}

Handle OakNode::AddChannel(std::unique_ptr<ChannelHalf> half) {
  absl::MutexLock lock(&mu_);
  Handle handle = NextHandle();
  channel_halves_[handle] = std::move(half);
  return handle;
}

bool OakNode::CloseChannel(Handle handle) {
  absl::MutexLock lock(&mu_);
  auto it = channel_halves_.find(handle);
  if (it == channel_halves_.end()) {
    return false;
  }
  channel_halves_.erase(it);
  return true;
}

ChannelHalf* OakNode::BorrowChannel(Handle handle) const {
  absl::ReaderMutexLock lock(&mu_);
  auto it = channel_halves_.find(handle);
  if (it == channel_halves_.end()) {
    return nullptr;
  }
  return it->second.get();
}

MessageChannelReadHalf* OakNode::BorrowReadChannel(Handle handle) const {
  absl::ReaderMutexLock lock(&mu_);
  auto it = channel_halves_.find(handle);
  if (it == channel_halves_.end()) {
    return nullptr;
  }
  ChannelHalf* half = it->second.get();
  auto value = absl::get_if<std::unique_ptr<MessageChannelReadHalf>>(half);
  if (value == nullptr) {
    return nullptr;
  }
  return value->get();
}

MessageChannelWriteHalf* OakNode::BorrowWriteChannel(Handle handle) const {
  absl::ReaderMutexLock lock(&mu_);
  auto it = channel_halves_.find(handle);
  if (it == channel_halves_.end()) {
    return nullptr;
  }
  ChannelHalf* half = it->second.get();
  auto value = absl::get_if<std::unique_ptr<MessageChannelWriteHalf>>(half);
  if (value == nullptr) {
    return nullptr;
  }
  return value->get();
}

bool OakNode::WaitOnChannels(std::vector<std::unique_ptr<ChannelStatus>>* statuses) const {
  while (true) {
    bool found_ready = false;
    bool found_readable = false;
    auto notify = std::make_shared<Notification>();
    for (uint32_t ii = 0; ii < statuses->size(); ii++) {
      uint64_t handle = (*statuses)[ii]->handle;
      MessageChannelReadHalf* channel = BorrowReadChannel(handle);
      if (channel == nullptr) {
        OAK_LOG(WARNING) << "{" << name_ << "} Waiting on non-existent read channel handle "
                         << handle;
        (*statuses)[ii]->status = ChannelReadStatus::INVALID_CHANNEL;
        continue;
      }

      ChannelReadStatus status = channel->ReadStatus(std::weak_ptr<Notification>(notify));
      (*statuses)[ii]->status = status;
      switch (status) {
        case ChannelReadStatus::READ_READY:
          OAK_LOG(INFO) << "{" << name_ << "} Message available on handle " << handle;
          found_ready = true;
          break;
        case ChannelReadStatus::ORPHANED:
          OAK_LOG(INFO) << "{" << name_ << "} Handle " << handle
                        << " is orphaned (no extant writers)";
          break;
        case ChannelReadStatus::NOT_READY:
          found_readable = true;
          break;
        default:
          OAK_LOG(ERROR) << "{" << name_ << "} Unexpected channel read status: " << status;
          return false;
          break;
      }
    }

    if (runtime_->TerminationPending()) {
      OAK_LOG(WARNING) << "{" << name_ << "} Node is pending termination";
      return false;
    }
    if (found_ready) {
      return true;
    }
    if (!found_readable) {
      OAK_LOG(WARNING) << "{" << name_ << "} No read-capable channels found";
      return false;
    }

    // Wait with a timeout to make end-of-day shutdown easier: this means that a
    // node with no pending work will still check termination_pending_
    // occasionally.
    notify->WaitForNotificationWithTimeout(absl::Seconds(1));

    // The only shared_ptr to the Notification object will be dropped here, at
    // which point any still-existing weak_ptr instances will no longer resolve.
  }
}

Handle OakNode::SingleHandle() const {
  absl::ReaderMutexLock lock(&mu_);
  if (channel_halves_.size() != 1) {
    return kInvalidHandle;
  }
  return channel_halves_.begin()->first;
}

}  // namespace oak
