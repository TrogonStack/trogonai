# Coding agent

Give a task and a repository. The CLI leases a sandbox, clones into a unique writable directory under `/tmp`, runs an AI SDK tool loop and prints the changed files and unified diff. It never commits or pushes. Progress lines go to stderr and are silenced with `NODE_ENV=production`; the scrubbed user report stays on stdout.

## Run

Use Node 22.7 or newer and pnpm from the `sandbox-examples` directory. The launcher adapts the native TypeScript flags to the installed Node version.

For a keyless, in-process run with no network:

```bash
SANDBOX_EXAMPLE_MODEL=scripted SANDBOX_PROVIDER=memory pnpm coding-agent "Greet the world" --repo https://example.invalid/demo.git
```

For a real Docker sandbox, configure the gateway variables from the [package setup](../../README.md#setup), preload the stock image on its Docker daemon and point the CLI at an OpenAI-compatible model endpoint:

```bash
docker pull buildpack-deps:bookworm-scm
SANDBOX_EXAMPLE_MODEL=<model id> SANDBOX_EXAMPLE_MODEL_BASE_URL=<endpoint url> SANDBOX_PROVIDER=local_docker pnpm coding-agent "Add a unit test for the parser" --repo https://github.com/owner/repo
```

The image supplies Git, and the sandbox needs egress to the repository host. The writable clone directory is independent of the image's default working directory.

## Settings

| Variable                         | Description                                                                                                                        |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `SANDBOX_EXAMPLE_MODEL`          | Required. A model id served by the endpoint below, or `scripted` to replay a fixed edit without a model call. There is no default. |
| `SANDBOX_EXAMPLE_MODEL_BASE_URL` | Base URL of an OpenAI-compatible API. Required unless the model is `scripted`.                                                     |
| `SANDBOX_EXAMPLE_MODEL_API_KEY`  | Bearer key for that endpoint, if it needs one. Unnecessary with `scripted`.                                                        |
| `SANDBOX_PROVIDER`               | `local_docker` by default, or `memory`. Empty and incompatible values fail before connecting.                                      |
| `GIT_TOKEN`                      | Optional private-repository credential delivered through the SDK's one-shot helper, never argv.                                    |
| `SANDBOX_GIT_FIXTURE_HOST`       | Host reachable from Docker for the gateway test's local repository. Defaults to `host.docker.internal`.                            |

The supplied OCI image is supported by the Docker path. Use a provider-specific image and workflow before extending the CLI to another provider.

## What memory mode fakes

`memory-repo.ts` seeds a tiny virtual repository. Nothing is fetched. Its Git handlers compare files with their clone-time contents and generate a whole-file diff; they do not implement a real index, ignore rules or renames. Its shell understands only `cat`, `ls` and `echo` with plain arguments.

The scripted model always edits `greeting.txt` from `Hello, there!` to `Hello, world!`. That proves the mechanics with the seeded repository or the local gateway fixture; it does not interpret arbitrary coding tasks.

## Verify

```bash
pnpm test:coding-agent
pnpm smoke
pnpm typecheck
```

The CLI tests exercise the real entry point from a root image cwd, provider validation before connection, cleanup, production stdout and model selection. The owned HTTP Git fixture uses the local Git executable and verifies a real shallow clone and read-only access.

With the gateway configured and the stock image preloaded:

```bash
pnpm test:gateway:coding-agent
```

The gateway test serves an owned temporary Git repository through Node and `git http-backend`, runs the scripted CLI through the real gateway, verifies the actual diff and releases every remaining observed lease. It closes the HTTP listener and deletes its temporary repository. It needs no external Git or model credentials.

## Output and lifetime

The tools are `run_command`, `read_file`, `write_file`, `edit_file` and `find_files`, rooted at the clone. Tool inputs and outputs, CLI configuration and final command/status/diff results use Zod contracts. The final report includes new files by marking them intent-to-add in the guest index. The sandbox is deleted at the end, so save the report if you need to keep the work.
