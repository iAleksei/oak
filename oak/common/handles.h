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

#ifndef OAK_COMMON_HANDLES_H_
#define OAK_COMMON_HANDLES_H_

#include <stdint.h>

#include "absl/base/attributes.h"

namespace oak {
using Handle = uint64_t;
ABSL_CONST_INIT extern const Handle kInvalidHandle;
}  // namespace oak

#endif  // OAK_COMMON_HANDLES_H_
