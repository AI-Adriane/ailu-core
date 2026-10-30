---
title: "Migrating named model credentials"
description: "Configure explicit grants for custom-endpoint credentials and TypeScript standalone provider-key overrides."
---

# Migrating named model credentials

The unreleased `apiKeyEnv` security fix introduces a **breaking default change**: an unset
`AILU_API_KEY_ENV_ALLOWLIST` no longer allows arbitrary named environment-variable reads.
An absent, empty, or blank policy denies every nonblank selector before reading its variable.
The Rust process resolver also denies an invalid UTF-8 policy.

## Who must migrate

| Caller | Affected configuration |
| --- | --- |
| Rust graph engine, including TypeScript/Python graph bindings and the C ABI | A custom `baseUrl` with a named `apiKeyEnv`. |
| TypeScript standalone models | A named `apiKeyEnv`, both on custom `baseURL` endpoints and on explicit provider overrides without `baseURL`. |
| Python `ailu.llm_complete` | `base_url` with a named `api_key_env`. Without `base_url`, this function does not use `api_key_env` to override the built-in provider variable. |

Automatic provider-key lookup, such as reading `OPENAI_API_KEY` when no selector is supplied,
keeps its existing behavior. A custom endpoint with no selector, or a blank selector, stays
keyless. This migration does not add selector support to callers that previously ignored it.

## Configure the embedding host

1. Inventory the selectors used by graphs and standalone model calls, including TypeScript
   provider overrides. List only credentials those callers are permitted to access.
2. Set the policy in the trusted host environment before model calls or graph construction:

   ```bash
   export AILU_API_KEY_ENV_ALLOWLIST="GATEWAY_KEY,CORP_OPENAI_KEY"
   export AILU_MODEL_ENDPOINT_ALLOWLIST="https://models.example.com/v1"
   ```

   These grants cover both of the following TypeScript declarations:

   ```ts
   model.openaiCompatible({
     baseURL: "https://models.example.com/v1",
     model: "llama-3",
     apiKeyEnv: "GATEWAY_KEY"
   });
   model.openai("gpt-4o", { apiKeyEnv: "CORP_OPENAI_KEY" });
   ```

3. Keep secret values in the host environment. An allowlist entry authorizes a name; it does
   not supply a value. An allowed but unset or empty key still produces the missing-key error.
4. Check an allowed call and a refused selector before rollout. Refusals use
   `AILU_API_KEY_ENV_NOT_ALLOWED` in Rust error text and TypeScript's
   `ApiKeyEnvNotAllowedError`; Python standalone calls raise `ailu.ApiKeyEnvNotAllowedError`.

Entries are comma-separated, trimmed, and case-sensitive. A trailing `*` grants a prefix, for
example `AILU_ENDPOINT_*`. A **bare `*` grants every name**, restoring unrestricted selection;
do not use it as a migration shortcut for untrusted graphs. See the
[matching rules](./models.md#restrict-the-variables-apikeyenv-may-name).

## Destination authorization is separate

The credential allowlist authorizes variable names, not endpoint origins. Custom endpoints
also require their exact base URL in `AILU_MODEL_ENDPOINT_ALLOWLIST`, including keyless calls
and replay construction. Configure both policies for keyed custom endpoints; leave the key
selector absent for a keyless endpoint while still authorizing its destination.

Replay still checks the custom destination grant, but its model gateway uses recorded responses
without resolving a credential or connecting to the provider. It needs neither a named-key grant
nor a provider key for that replay.

Destination authorization runs before graph-selected credential lookup. Automatic provider-key
lookup and host-configured Ollama/LM Studio routes keep their existing configuration. See
[authorizing custom destinations](./models.md#authorize-custom-destinations) and the
[custom endpoint upgrade guide](./upgrading-2-8.md#custom-endpoint-policy) for URL matching rules.
