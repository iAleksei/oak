# Oak Development

## Prerequisites

We use Docker to install the base dependencies that need to exist at the system
level, e.g. the [Intel SGX SDK](https://software.intel.com/en-us/sgx/sdk) and
the [Rust compiler](https://www.rust-lang.org/tools/install); these steps are
detailed in [`Dockerfile`](/Dockerfile). See
https://docs.docker.com/engine/reference/builder/ for more information.

Inside Docker, we use Bazel to version, install and build any non-Rust
dependencies, including our own code. Dependencies are listed in
[`WORKSPACE`](/WORKSPACE). See
https://docs.bazel.build/versions/master/external.html for more information.

To set up your development environment, you need the following applications. For
the accurate versions required for a successful build please consult the current
[`Dockerfile`](/Dockerfile).

- Docker:
  - Follow install instructions in https://docs.docker.com/install
- Bazel:
  - Follow install instructions in
    https://docs.bazel.build/versions/master/install.html
- Rust:
  - Install instructions from https://rustup.rs/:
    - `curl https://sh.rustup.rs -sSf > /tmp/rustup`
    - `less /tmp/rustup` (inspect downloaded script before running it)
    - `sh /tmp/rustup` (follow on-screen instructions -- option 1 is fine to
      start with)
    - add `source $HOME/.cargo/env` to your shell init script (e.g. `.bashrc` or
      `.zshrc`)
  - Add WebAssembly target to be able to compile to WebAssembly (see
    [Rust Platform Support](https://forge.rust-lang.org/release/platform-support.html)):
    - `rustup target add wasm32-unknown-unknown`
- Protocol Buffers:
  - Install protobuf-compiler: `apt install protobuf-compiler`

[Step by step instructions for installing Oak on Ubuntu 18.04](/INSTALL.md)
shows how to install the prerequisites starting off with a clean Ubuntu install.

Note the server runs in the Docker container but the examples run on the host
machine. This means you might be missing other dependencies like the `protoc`
protocol compiler.

## Run Example Application

The following command runs both an Oak server (as a background process) and an
Oak application client:

```bash
./scripts/docker_run ./scripts/run_example -e hello_world
```

This command consists of the following steps performed inside Docker:

### Build Application

The following command compiles the code for an example Oak Application from Rust
to a WebAssembly module and then serializes it into a binary application
configuration file to be loaded to the Oak Server:

```bash
./scripts/build_example -e hello_world
```

This binary application configuration file includes the compiled Wasm code for
the Oak Application, embedded in a serialized protocol buffer that also includes
the Application's configuration.

### Run Server

The following command builds and runs an Oak Server instance inside Docker,
running a specific Oak Application.

```bash
./scripts/run_server -e hello_world
```

The Oak Server can also be built in debug mode, as well as using any of the
sanitizers Clang supports (e.g.
[asan](https://clang.llvm.org/docs/AddressSanitizer.html),
[tsan](https://clang.llvm.org/docs/ThreadSanitizer.html) etc.). Details about
available sanitizers can be found in the [`.bazelrc`](/.bazelrc) file.

The following command builds and run Oak Local Server with tsan enabled. Replace
`tsan` with other configurations for different sanitisers:

```bash
./scripts/run_server -s tsan -e hello_world
```

### Run Client

The following command (run in a separate terminal) compiles the code for the
client of an example Oak Application, and runs it locally. It works with both
Servers (Docker and Dev):

```bash
./scripts/run_example -s none -e hello_world
```
