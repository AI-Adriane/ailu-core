# ADR 0009 — Clerk for identity (login), Ailu keeps tenancy + RBAC

- Status: **Accepted** (control plane — private)
- Date: 2026-06-20

## Context

The control plane already ships a working custom auth: HS256 session-JWTs backed by a
revocable session store, an invite flow, and a server-resolved multi-tenant RBAC
(`memberships` = source of truth for role). The gap is **identity UX**: no hosted login UI,
no social/SSO/MFA, and no first-user bootstrap. Building those well is non-trivial.

The `auth.module` already anticipated a provider swap ("swap in an OAuth provider here").

## Decision

Use **Clerk for identity only** (authentication, hosted Sign-in/Sign-up, social, MFA, reset).
Ailu **keeps** its tenancy + roles + invitations — Clerk does not own the tenant model.

- **Studio**: `@clerk/nextjs` (`ClerkProvider`, `clerkMiddleware`, hosted `<SignIn>/<SignUp>`).
  The API client attaches the Clerk session token as `Authorization: Bearer`.
- **API**: `JwtAuthGuard` is unchanged; the seam is `AuthService.validateToken`, which now
  tries the **native** session token first, then falls back to **Clerk** (`@clerk/backend`
  `verifyToken`, JWKS-verified, issuer/exp checked). Native login + invites keep working.
- **Provisioning**: on the first request from an unknown `clerkUserId`, create a `users` row
  (linked by `clerkUserId`, `passwordHash` NULL), a tenant (`TenantService.ensureTenant`,
  free plan) and an **owner** membership (`assignUser`). Role is still resolved server-side
  from `memberships` — never from a Clerk claim.
- **Schema** (`db:push`): `users.clerk_user_id` (unique, nullable); `users.password_hash`
  becomes **nullable** (Clerk users have none — native login then rejects them by design).
- **Unchanged**: `WorkerAuth` (m2m token), `@Roles`, invitations, the session store.
- **Env** (read directly from `process.env`, NOT the engine config schema — product concern,
  like Stripe/PII): `CLERK_SECRET_KEY` (API) + `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY` (Studio).

## Security

- Clerk tokens are verified against Clerk's JWKS (RS256) with issuer/expiry checks; we never
  trust unverified claims. Role/tenant are resolved from `memberships`, server-side.
- `passwordHash` nullable: the password credentials provider must reject a null hash (no
  native login for Clerk-only users).
- US data residency (Clerk) conflicts with the EU-sovereignty line (Paddle/Mollie) — accepted
  for DX now; a self-hosted OIDC (Logto/Keycloak) is the sovereign alternative behind the same
  `validateToken` seam.

## Reversibility

The seam is `validateToken` + the `CredentialsProvider`. Dropping Clerk = remove the fallback
branch; swapping to another IdP = one new verifier behind the same seam. The tenant/role model
is untouched either way.
