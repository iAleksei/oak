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

#ifndef OAK_SERVER_NODE_H_
#define OAK_SERVER_NODE_H_

#include <atomic>
#include <memory>
#include <random>
#include <unordered_map>
#include <vector>

#include "absl/base/thread_annotations.h"
#include "absl/synchronization/mutex.h"
#include "oak/common/handles.h"
#include "oak/server/base_runtime.h"
#include "oak/server/channel.h"

namespace oak {

class OakNode {
 public:
  OakNode(BaseRuntime* runtime, const std::string& name)
      : runtime_(runtime), name_(name), prng_engine_() {}
  virtual ~OakNode() {}

  virtual void Start() = 0;
  virtual void Stop() = 0;

  // Take ownership of the given channel half, returning a channel handle that
  // the node can use to refer to it in future.
  Handle AddChannel(std::unique_ptr<ChannelHalf> half) LOCKS_EXCLUDED(mu_);

  // Close the given channel half.  Returns true if the channel was found and closed,
  // false if the channel was not found.
  bool CloseChannel(Handle handle) LOCKS_EXCLUDED(mu_);

  // Return a borrowed reference to the channel half identified by the given
  // handle (or nullptr if the handle is not recognized).  Caller is responsible
  // for ensuring that the borrowed reference does not out-live the owned
  // channel.
  ChannelHalf* BorrowChannel(Handle handle) const LOCKS_EXCLUDED(mu_);
  MessageChannelReadHalf* BorrowReadChannel(Handle handle) const LOCKS_EXCLUDED(mu_);
  MessageChannelWriteHalf* BorrowWriteChannel(Handle handle) const LOCKS_EXCLUDED(mu_);

  // Wait on the given channel handles, modifying the contents of the passed-in
  // vector.  Returns a boolean indicating whether the wait finished due to a
  // channel being ready (true), or a failure (false, indicating either node
  // termination or no readable channels found).  Caller is responsible for
  // ensuring that none of the waited-on channels are closed during the wait
  // operation.
  bool WaitOnChannels(std::vector<std::unique_ptr<ChannelStatus>>* statuses) const;

 protected:
  // If the Node has a single registered handle, return it; otherwise, return
  // kInvalidHandle. This is a convenience method for initial execution of a
  // Node, which should always start with exactly one handle (for a read half)
  // registered in its channel_halves_ table; this handle is passed as the
  // parameter to the Node's oak_main() entrypoint.
  Handle SingleHandle() const LOCKS_EXCLUDED(mu_);

  // Runtime instance that owns this Node.
  BaseRuntime* const runtime_;

  const std::string name_;

 private:
  Handle NextHandle() EXCLUSIVE_LOCKS_REQUIRED(mu_);

  using ChannelHalfTable = std::unordered_map<Handle, std::unique_ptr<ChannelHalf>>;

  mutable absl::Mutex mu_;  // protects prng_engine_, channel_halves_

  std::random_device prng_engine_ GUARDED_BY(mu_);

  // Map from channel handles to channel half instances.
  ChannelHalfTable channel_halves_ GUARDED_BY(mu_);
};

}  // namespace oak

#endif  // OAK_SERVER_NODE_H_
