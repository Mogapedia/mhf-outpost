# MHF IELess Launcher CLI

Command-Line Interface for `mhf-iel`.

## Usage

1. Get a `mhf-iel-cli.exe` file by either [compiling the project](../README.md) or downloading the [latest release](https://github.com/rockisch/mhf-iel/releases/).
2. Download [`config.example.json`](config.example.json).
3. Copy `config.example.json` to `config.json` and edit it with your server configuration.
4. Copy both `config.json` and `mhf-iel-cli.exe` to your MHF folder.
5. Run `mhf-iel-cli.exe`.

If you plan on using the CLI interface as the entrypoint of your external application, run `mhf-iel-cli.exe --help` to see extra options available.

## Configuration

### Where Config Values Come From

**Important:** In a production setup, you should NOT manually create `config.json`. Instead, these values should come from your MHF server's API:

1. **User Authentication**: Your launcher GUI calls the server's `/login` or `/register` endpoint with username/password
2. **Server Response**: The server returns authentication token, character list, server connection details, and session data
3. **Pass to CLI**: Your launcher passes this data to `mhf-iel-cli.exe` via `--config-data` or writes it to `config.json`

See the [Python GUI example](../gui.py) in the parent directory for a reference implementation.

### Manual Configuration (Development/Testing Only)

For development or testing purposes, you can manually create a `config.json` file. The file contains all settings needed to connect to your MHF server. Here's what each field means:

### Character Settings

- `char_id`: Your character ID (obtained from server).
- `char_name`: Your character name.
- `char_new`: Set to `true` for new characters, `false` otherwise.
- `char_hr`: Hunter Rank (0-999).
- `char_gr`: Guild Rank (1-50).
- `char_ids`: Array of available character IDs for this account.

### User Authentication

- `user_token_id`: Token identifier (number).
- `user_token`: Authentication token (must be exactly 16 characters).
- `user_name`: Your username.
- `user_password`: Your password.
- `user_rights`: User permission level (typically 12 for regular users).

### Server Connection

- `server_host`: Server hostname or IP address (e.g., `"127.0.0.1"` for localhost).
- `server_port`: Server port number (e.g., `53312`).

### Session Settings

- `entrance_count`: Login counter.
- `current_ts`: Current timestamp (0 for default).
- `expiry_ts`: Session expiration timestamp (4294967295 for max).

### Notices

- `notices`: Array of in-game notice messages.
  - `data`: Message content in HTML-like format.
  - `flags`: Message flags (0 for default).

### Mezeporta Festival Settings

- `mez_event_id`: Current Mez Fes event ID.
- `mez_start`: Event start timestamp.
- `mez_end`: Event end timestamp.
- `mez_solo_tickets`: Number of solo tickets.
- `mez_group_tickets`: Number of group tickets.
- `mez_stalls`: Array of available festival stalls (e.g., `["TokotokoPartnya", "Pachinko"]`).

### Version

- `version`: Game version - `"ZZ"` for MHF-Z/ZZ, or `"F5"` for MHF-F5.

### Optional Fields

- `mhf_folder`: Path to MHF installation (auto-detected if omitted).
- `mhf_flags`: Array of CLI flags to pass to the game.
