# Heart Rate Monitor

This rust program connects to a heart rate monitor (worn on the chest or somewhere else) via BluetoothLowEnergy.\
The data can then be queried via HTTP or logged to a csv file.

## Features

- The measured heart rate can be logged to a csv file.
- The measured heart rate can be queried via HTTP requests.
- The measured heart rate can be obtained using a websocket.
- Multiple instances of this program can run on the same device (requires multiple heart rate monitors to connect to).
- Each instance of this program can only connect to one heart rate monitor at a time.
- The program can render [Tera templates](https://github.com/Keats/tera) to show the heart rate.
- The program will connect automatically to any already known device on startup, if found.
- Can be extended to work with heart rate monitors, which do not care about standards (see [Extensions](#extensions))

## Configuration

The environment variable `RUST_LOG` controls the log behaviour of the program.

### Command line arguments

Command line arguments (if provided) will overwrite settings of the configuration file.

The following command line arguments exist:

- `enable-http-server`
    - type: boolean
    - if the HTTP server should be enabled
- `http-port`
    - type: integer
    - the HTTP server port
- `enable-csv-log`
    - type: boolean
    - if the data should be logged as csv file
- `accept-new-device`
    - type: flag (has no parameters)
    - default: `false`
    - if the program should try to connect and pair to the device with mac `hrm_mac` without asking for confirmation
- `hrm-mac`:
    - type: string (mac address)
    - mac of the herat rate monitor to search for
    - overrides `hrm_index`
- `hrm-index`:
    - type: integer
    - index of the herat rate monitor to connect to; order in configuration file is mandatory
    - first device has index 1
    - overridden by `hrm_mac`
    - this will not pair the device if it is unknown
- `pin-device`:
    - type: boolean
    - remember the used device for reconnections; do not ask the user each time
    - this will not connect to a device after startup, only for reconnects
- `noninteractive-rescan`:
    - type: flag (has no parameters)
    - default: `false`
    - perform an automatic rescan for devices, if no device is found matching the criteria
    - this has no effect for the initial connection after startup (only reconnections)
- `debug-device`
    - type: flag (has no parameters)
    - default: `false`
    - will spit out A LOT of stuff while the device is connected
    - this will deactivate logging to file and the HTTP server

### Configuration file

The program expects a file called `settings.json` in the same folder as the program file. If this file does not exist, a
file with default values will be created when the first heart rate monitor is connected.\
[settings_example.json](settings_example.json) provides an example config file, which can be used as reference.

The program expects the following config options:

| name                   | type                       | default     | description                                                    |
|------------------------|----------------------------|-------------|----------------------------------------------------------------|
| `hrm_list`             | `list of HeartRateMonitor` | `[]`        | A list of known heart rate                                     |
| `http_port`            | `integer`                  | `8080`      | Port the HTTP server should listen on                          |
| `http_host`            | `string`                   | `127.0.0.1` | Host the HTTP server binds to                                  |
| `enable_http_server`   | `boolean`                  | `false`     | If the HTTP server should be enabled at all                    |
| `http_template_folder` | `string`                   | `null`      | A folder which contains the Tera templates for the HTTP server | 
| `enable_csv_log`       | `boolean`                  | `false`     | If the csv logger should be enabled                            |
| `csv_folder`           | `string`                   | `null`      | A folder to put the csv files into                             |

All config values, which do not have a default are required.\
Default of `null` means, that the value is not set (and is optional).

A HeartRateMonitor looks like this:

| name         | type     | description                                                                        |
|--------------|----------|------------------------------------------------------------------------------------|
| `name`       | `string` | Name of the device shown in user interface; has no meaning for the matching itself |
| `mac`        | `string` | Bluetooth mac address of the device; this is used to search for known devices      |

## HTTP

### Routes

- `/`: presents a general overview of possible HTTP routes
- `/hear_rate`: returns the actual [HeartRate](#heartrate-data) as JSON (see below)
- `/data`: returns the actual [HeartRate](#heartrate-data)  as JSON (see below)
- `/template`: renders the [template](#templates) given as `name` query parameter or `default.html` with the actual data
- `/reload_templates`: reloads all available templates without restarting the program
- `/list_templates`: lists all loaded templates
- `/ws`: initiates a [websocket](#websocket) connection (see below)
- `/websocket`: initiates a websocket connection (see below)

### Websocket
After opening a connection, the client will receive a message as json, every time the heart rate monitor provides an update.
This message contains [HeartRate Data](#heartrate-data).

### Templates

- The HTTP server can render templates, when the `/templates` route is called.
- The template must be loaded at this time to be rendered.
- The server will load all templates, which are present in the `http_template_folder` and have one of these file
  endings:
    - `html`
    - `html.tera`
    - `html`
    - `htm.tera`
- During rendering of a template, the following variables are available:
    - `hr_disc` is true, if the program is not connected to any heart rate monitor
    - `hr_val` is the actual heart rate value in bpm
    - `hr_connected`: if the heart rate monitor has contact to the skin, this is true
    - `hr_battery`: remaining battery of the heart rate monitor in %
    - Only `hr_disc` is always, present; if no device is connected, the other ones are missing

### HeartRate Data

The heart rate data provided by the api or in the websocket messages is a json object, which looks like this:

```json lines
{
  "timestamp": "2024-11-12T00:09:19.161812912Z",
  "hr_state": HrState
}
```

HrState is either\
`disconnected` or

```json lines
{
  "ok": {
    "hr": 76,
    // the actual heart rate
    "contact_ok": true,
    // if the device has skin contact
    "battery": 100
    // battery level in %
  }
}
```

## Building
### Native
1. Have `rust` and `cargo` installed.
2. Run `cargo build` or `cargo build --release`.
3. Run your compiled program (you will find it in the `target` directory).
### Cross compilation under Linux for Windows
1. Have `podman` and `cargo` installed.
2. Install `cross` via `cargo`.
3. Run `CROSS_CONTAINER_ENGINE=podman NIX_STORE="/tmp/empty" cross build --release --target x86_64-pc-windows-gnu`
4. Search for your compiled program somewhere, lol.

## Extensions
This program can be extended to allow connections to heart rate monitors, which do not care about standards.\
To do so, you must be familiar with rust.\
1. Use the debug output of the program and connect to any device you want to add.
2. Observe and reverse engineer your device.
3. Build a fitting adaptor and put it in `src/adaptors`.
4. Register your new adaptor in `src/adaptors/mod.rs` in `ADAPTORS`. 

If you need to force a device to use a specific adaptor, add the `adaptor_id` config option in the config for this  `HeartRateMonitor` (see above). 
The value should be the adaptor id as set in `src/adaptors/mod.rs` in `ADAPTORS` (first argument).

## Credits
