# Script examples

These examples use Python 3's standard library. Read the scripts, copy the
ones you want into `~/.config/slate/scripts`, and add their configuration to
`~/.config/slate/config.toml`. Use absolute paths in `argv`: Slate does not
expand `~` or environment variables. Replace `/home/YOU` below with your home
directory. Restart Slate after changing registered scripts.

| Example | Purpose | Input | Output |
| --- | --- | --- | --- |
| `uppercase.py` | Uppercase a visual selection | selection | replace-selection |
| `format_json.py` | Pretty-print selected JSON | selection | replace-selection |
| `meeting.py` | Insert a meeting template | none | insert |
| `word_count.py` | Count words, characters and lines without editing | note | message |
| `rates.py` | Fetch currency rates from a provider | empty request | rates JSON |
| `rates_fixed.py` | Learn currency conversion with invented offline rates | empty request | rates JSON |

## Currency rates

From the repository root:

```sh
mkdir -p ~/.config/slate/scripts
cp scripts/examples/rates.py ~/.config/slate/scripts/
```

Add this as its own TOML section:

```toml
[currency]
argv = ["python3", "/home/YOU/.config/slate/scripts/rates.py", "EUR"]
timeout_seconds = 30
```

Slate loads cached rates first and attempts one async fetch at startup. Use
`:currency refresh` for another fetch; there is no periodic refresh. With
startup background tasks disabled, only explicit refreshes fetch rates.
The final `EUR` argument selects the provider's base currency, and can be
changed to `USD` or another supported code.

Try these lines in a note with math enabled:

```text
10 USD to EUR
2000 RSD to EUR
budget := 100 EUR to USD
budget * 2
```

`rates.py` uses the [ExchangeRate-API open endpoint](https://www.exchangerate-api.com/docs/free).
Rates by [ExchangeRate-API](https://www.exchangerate-api.com); see the provider's
terms and attribution requirements. This script needs network access.

For a deterministic offline example, copy `rates_fixed.py` instead and change
`argv` to `["python3", "/home/YOU/.config/slate/scripts/rates_fixed.py"]`.
Its rates are invented: `10 USD to EUR` yields `8 EUR`, and `100 EUR to USD`
yields `125 USD`. Edit the rate dictionary to learn how the results change,
then run `:currency refresh`.

Currency scripts print a different response from registered scripts:

```json
{"base":"EUR","rates":{"USD":1.25,"RSD":117.0}}
```

Each value is units of that currency per one unit of `base`. `as_of` is an
optional date string. Currency scripts are configured under `[currency]`,
and cannot be invoked with `:run`. To inspect their raw output in a shell:

```sh
python3 ~/.config/slate/scripts/rates.py EUR
python3 ~/.config/slate/scripts/rates_fixed.py
```

## Registered scripts

Copy the examples you want, then register them:

```sh
mkdir -p ~/.config/slate/scripts
cp scripts/examples/uppercase.py scripts/examples/format_json.py \
  scripts/examples/meeting.py scripts/examples/word_count.py \
  ~/.config/slate/scripts/
```

```toml
[scripts.uppercase]
argv = ["python3", "/home/YOU/.config/slate/scripts/uppercase.py"]
input = "selection"
output = "replace-selection"

[scripts.format-json]
argv = ["python3", "/home/YOU/.config/slate/scripts/format_json.py"]
input = "selection"
output = "replace-selection"

[scripts.meeting]
argv = ["python3", "/home/YOU/.config/slate/scripts/meeting.py"]
input = "none"
output = "insert"

[scripts.word-count]
argv = ["python3", "/home/YOU/.config/slate/scripts/word_count.py"]
input = "note"
output = "message"
```

- Select text with `v` or `V`, then run `:run uppercase`.
- Select `{"name":"Slate","items":[1,2]}`, then run `:run format-json`.
  Use `:run format-json 4` for four-space indentation. Invalid JSON keeps
  the original selection and reports an error.
- Run `:run meeting 'Project X'` to insert a template at the cursor.
- Run `:run word-count` to report counts for the current note, including
  unsaved edits. `str.split()` defines words; `str.splitlines()` defines lines.

Transforms and inserts are undoable. `output = "message"` only changes the
status message. Registered scripts read a v1 request from stdin and print
one `{"text":"...","message":"..."}` response; `text` is always required.
Arguments appear in both the request's `args` and the process argument list.

To test the response protocol before registering an example:

```sh
printf '%s' '{"version":1,"args":[],"text":"hello world","note_id":null}' |
  python3 scripts/examples/word_count.py
printf '%s' '{"version":1,"args":["4"],"text":"{\"a\":1}","note_id":null}' |
  python3 scripts/examples/format_json.py
```

Once registered, the CLI accepts piped text without modifying any note:

```sh
printf 'hello world' | slate run word-count
printf '{"a":1}' | slate run format-json 4
```

See [the scripting guide](../../docs/scripting.md) for keybindings, CLI targets,
cancellation and the complete protocol.
