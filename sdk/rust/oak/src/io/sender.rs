//
// Copyright 2020 The Project Oak Authors
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

use crate::io::Encodable;
use crate::{OakError, OakStatus, WriteHandle};
use serde::{Deserialize, Serialize};

/// Wrapper for a handle to the send half of a channel, allowing to send data that can be encoded as
/// bytes + handles via the `Encodable` trait.
///
/// For use when the underlying [`Handle`] is known to be for a send half.
///
/// [`Handle`]: crate::Handle
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sender<T: Encodable> {
    pub handle: WriteHandle,
    phantom: std::marker::PhantomData<T>,
}

impl<T: Encodable> Sender<T> {
    pub fn new(handle: WriteHandle) -> Self {
        Sender {
            handle,
            phantom: std::marker::PhantomData,
        }
    }

    /// Close the underlying channel used by this sender.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn close(&self) -> Result<(), OakStatus> {
        crate::channel_close(self.handle.handle)
    }

    /// Attempt to send a value on this sender.
    ///
    /// See https://doc.rust-lang.org/std/sync/mpsc/struct.Sender.html#method.send
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn send(&self, t: &T) -> Result<(), OakError> {
        let message = t.encode()?;
        crate::channel_write(self.handle, &message.bytes, &message.handles)?;
        Ok(())
    }
}
