# GEO-RPC

this is an RPC server and client that integrates the geometric and stereoscopic
validation processes to be used remotely for a distributed validation system.
The server and client are both written in Rust and tarpc as the framework for
managing RPC communication.

## Requirements

at the time of developement the latest version of Rust (1.93.0) was being used
and tested against. all dependencies are specified in the individual modules for
the client, server, and com packages. no other build requirements are needed
outside of what Rust and Cargo provide by default.

the server requires running in a linux system as it will attempt to directly
execute commands that are specific to linux systems. this may work on mac's but
has not been tested and this will not work on windows systems.

## Packages

there are 3 different packages that are available and can be found in the
`modules` directory.

1. `client`: contains all client specific code for connecting and iteracting
   with RPC servers.
2. `server`: contains all server specific code for managing the local device and
   handling incoming requets from clients.
3. `com`: contains all shared code that for the client and server. mainly
   request and response structs as well as the trait for defining the RPC
   requests available.

each module specifies dependencies and workspace dependencies that can be found
in their respecitve `Cargo.toml`.

## Running

### Server

the server will require a TOML config file to define what scripts and binaries
it needs to call for the geometric and stereoscopic validation processes and
anything else needed for server operation. and example config file can look like
this:

```toml
# this is not required as the server will default to listening for all
# connections on port 6789
listen = "192.168.0.1:1234"

[exec]
cameras = "path/to/cameras/json/for/device"
background = "path/to/build-backgrounds/binary"
validator = "path/to/geometric/validator/binary"

[exec.stl_render]
binary = "path/to/specific/python/binary"
script = "path/to/python/script/to/load"

# any additional static arguments to apply that are not defined at runtime, if
# there are no additional arguments then you don't need to have this
[[exec.stl_render.args]]
flag = "--specific-flag"
# value is optional if you only need a flag or if the flag includes the value
# like having "--flag=value"
value = "optional_value_if_needed"

# the stereopsis is optional if the server does not need to run the code for its
# cameras for whatever reason
[exec.stereopsis]
binary = "path/to/specific/python/binary"
script = "path/to/python/script/to/load"

# similar to the as the stl-render
[[exec.stereopsis.args]]
flag = "--additional"
value = "maybe_needed"
```

the server will also require the config json for the cameras available to the
device as it will feed arguments to the validators based on the config. below is
an example of the json and the required fields:

```json
{
    "cam_1": {
        "serial": "12345",
        "position": "left",
        "full_frame_output_dir": "output/path/for/frame/data"
    },
    "cam_2": {
        "serial": "23456",
        "position": "right",
        "full_frame_output_dir": "output/path/for/frame/data"
    }
}
```

the serial number corresponse to the serial number of the camera device and is
currently just for tracking and pulling information for the client to inspect.

the stereopsis will need at least 2 cameras and have them define a left and
right position for the 2. if there are more than one or if the left and right
have not bee specified or more that one camera has been specified as left or
right then the server will complain.

once the config files have been specified all that is needed to run the server
is running the following command. it will attempt to look for the toml config
file in the current working directory (`config.toml` or `config.ignore.toml`)
otherwise you will need to specify the file manually:

```
RUST_LOG=server=trace cargo run --release -p server
```

the `RUST_LOG` for logging additional information to the terminal if desired
further details can be looked up on how to specify that in `tracing_subscriber`
package. this will run the server in a release build using the default listen
address and log all server specific `trace` events along with `error`, `warn`,
`info`, and `debug` log events.

additonal command line options for the server can be found by running

```
cargo run --release -p server -- --help
```

### Client

the client only requires a single server to talk to or a file with a list of
servers to connect to if desired. all commands for the client can be listed by
running:

```
cargo run --release -p client -- --help
```

the format for the nodes file is as follows:

```
# any line starting with a # will be ignored. the file can contain a valid ipv4
# or ipv6 address as well as a port number if the server is not using the 
# default port value.
192.16.0.4
[::1]:1234
```

## Note

a lot of naming is a mess as some of it was quick or made sense at time of
writing. there are still edge cases that have not been ironed out so both the
client and server will still have bugs more than likely.
