# niminal in Docker

`docker-compose.yml` at the repo root defines one service (`niminal`) that:

- builds an image (`niminal:dev`) with Nim, Nimble, git and Python 3 (the
  test suite spawns `python3` fixture servers),
- mounts the repo at `/workspace`, so code changes are picked up by every
  `docker compose run` without rebuilding the image,
- mounts the sibling `../nimgent` package at `/nimgent` so
  `--path:"../nimgent/src"` from `/workspace` resolves,
- forwards `OPENROUTER_API_KEY`, `OPENAI_API_KEY`, and `ANTHROPIC_API_KEY` from your shell
  environment into the container, and
- stays running (`sleep infinity`) so you can open a shell in it.

## Up

```sh
docker compose up -d        # build if needed, leave the container Up
docker compose exec niminal bash
```

`docker compose up` (foreground) also stays attached and idle; exec a
shell from another terminal the same way. One-shot commands still work
via `run`, which replaces the idle command:

```sh
docker compose run --rm niminal nimble build
docker compose run --rm niminal nimble test
docker compose run --rm -it niminal ./niminal
```

`nimble test` compiles and runs `tests/all_tests.nim`, which launches the
`python3` fixture servers from inside the container. Pass `-it` for the
TUI so keybindings work.

## Env vars

Any host environment variable listed in `docker-compose.yml` under
`environment` is passed through to the container. A common invocation:

```sh
OPENROUTER_API_KEY=sk-or-... docker compose run --rm -it niminal ./niminal
```

To add another variable, add its name (unchanged) to the `environment`
list in `docker-compose.yml`, e.g.:

```yaml
    environment:
      - OPENROUTER_API_KEY
      - OPENAI_API_KEY
      - ANTHROPIC_API_KEY
      - ANY_OTHER_VAR
```

Variables that aren't set on the host are omitted. Niminal also reads
`~/.niminal/config.json`, `~/.niminal/AGENTS.md`, `~/.niminal/skills/`,
`~/.niminal/sessions/` (via `$HOME`); inside the container `$HOME` points
to a throwaway directory, so per-project config stays in the mounted repo
(`.niminal/`), and sessions are ephemeral across containers. To persist
them, mount a host directory at `/workspace-volatile`, e.g. add to
`docker-compose.yml`:

```yaml
    volumes:
      - .:/workspace
      - ~/.niminal:/workspace-volatile
```

## One-shot shell

```sh
docker compose run --rm -it niminal bash
```