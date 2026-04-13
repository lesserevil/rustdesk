# 12 - Security Analysis

**Audience**: All developers, security reviewers

## Threat Model

### Trust Boundaries

```mermaid
flowchart TD
    subgraph TB1["Trust Boundary 1: Physical Security"]
        KEY[Physical Security Key]
        USER[User at Local Machine]
    end

    subgraph TB2["Trust Boundary 2: Local Machine (Linux/Windows/macOS)"]
        CLIENT["RustDesk Client\n(native or web + companion)"]
        HIDAPI["hidapi\n(Linux: hidraw, Windows: HID API, macOS: IOKit)"]
    end

    subgraph TB3["Trust Boundary 3: Network"]
        TUNNEL[RustDesk Encrypted Channel]
    end

    subgraph TB4["Trust Boundary 4: Remote Machine (Linux/Windows/macOS)"]
        SERVER[RustDesk Server]
        VDEV["Virtual FIDO Device\n(Linux: uhid, Win: VHF, macOS: DriverKit)"]
        BROWSER[Browser]
        RP[Relying Party Website]
    end

    USER --> KEY
    KEY <--> HIDAPI
    HIDAPI <--> CLIENT
    CLIENT <--> TUNNEL
    TUNNEL <--> SERVER
    SERVER <--> VDEV
    VDEV <--> BROWSER
    BROWSER <--> RP
```

### Assets Under Protection

| Asset | Value | Location |
|-------|-------|----------|
| Private key material | Critical | Never leaves the physical key (hardware-bound) |
| CTAP2 assertions (signatures) | High | Transit through tunnel — replay-resistant by design |
| User presence confirmation | High | Physical touch on local machine |
| RP credentials (cookies, sessions) | High | On remote browser — not touched by CTAP feature |
| RustDesk session encryption key | High | In memory on both machines — pre-existing |

### Threat Analysis

#### T1: Man-in-the-Middle on the Tunnel

**Threat**: An attacker intercepts CTAP messages in transit and modifies them.

**Mitigation**: RustDesk's existing connection is encrypted with a symmetric key
established via public-key exchange during session setup (see `Client::secure_connection()`
in `src/client.rs:758`). CTAP messages travel inside this encrypted channel.

**Residual risk**: If the RustDesk encryption is compromised, CTAP messages are
also exposed. However, CTAP2 assertions are cryptographically bound to the
`rpIdHash` and `clientDataHash` — an attacker cannot modify these without the
authenticator rejecting them.

**Severity**: Low (defense in depth from both RustDesk encryption and CTAP2 crypto).

#### T2: Authenticator Reset via Remote

**Threat**: A malicious remote operator sends `authenticatorReset` (0x07) through
the tunnel, wiping all credentials from the user's security key.

**Mitigation**: The remote CTAP service (Component 06) **MUST** filter CTAP2
command bytes and reject `authenticatorReset` (0x07). This is implemented in
`handle_uhid_report()`:

```rust
if !msg.payload.is_empty() && msg.payload[0] == 0x07 {
    log::warn!("Blocked authenticatorReset command from remote");
    send_error(device, msg.cid, ERR_INVALID_CMD)?;
    return Ok(());
}
```

**Residual risk**: None — reset is blocked at the tunnel entry point.

**Severity**: Critical if unmitigated, None with filter.

#### T3: Credential Management Abuse

**Threat**: Remote operator uses `authenticatorCredentialManagement` (0x0A) to
enumerate or delete credentials stored on the key.

**Mitigation**: The remote service SHOULD also block command 0x0A by default.
Credential management is an administrative operation that should only be
performed locally.

**Recommended filter**: Block commands 0x07 (Reset), 0x09 (BioEnrollment),
0x0A (CredentialManagement), and 0x0D (Config). Only allow:
- 0x01 (MakeCredential)
- 0x02 (GetAssertion)
- 0x04 (GetInfo)
- 0x06 (ClientPIN) — needed for PIN entry
- 0x08 (GetNextAssertion)
- 0x0B (Selection) — harmless

```rust
const ALLOWED_CTAP2_COMMANDS: &[u8] = &[0x01, 0x02, 0x04, 0x06, 0x08, 0x0B];

fn is_allowed_ctap2_command(payload: &[u8]) -> bool {
    payload.first()
        .map(|cmd| ALLOWED_CTAP2_COMMANDS.contains(cmd))
        .unwrap_or(false)
}
```

**Severity**: High if unmitigated, None with filter.

#### T4: Phishing Amplification

**Threat**: A compromised remote machine shows a fake website (e.g., phishing
`g00gle.com` instead of `google.com`) and tricks the user into authenticating.

**Analysis**: This threat exists with or without CTAP passthrough — it's inherent
to remote desktop. The user is already trusting the remote machine's display.
CTAP passthrough does not make this worse because:

- The `rpId` in the CTAP2 request is bound to the actual origin the browser
  navigated to. If the phishing site has a different origin, the credential
  won't match.
- The user can see the remote browser's URL bar (assuming the session is visible).

**Mitigation**: User awareness. The "Tap your security key" prompt could
optionally display the `rpId` extracted from the CTAP2 request (requires parsing
the CBOR payload, which we otherwise treat as opaque — trade-off between security
and complexity).

**Severity**: Medium (inherent to remote desktop, not specific to this feature).

#### T5: Denial of Service via Rapid Requests

**Threat**: The remote side floods CTAP requests, causing constant prompts on
the local machine or hogging the security key.

**Mitigation**: The single-transaction design (only one active CBOR request at
a time) naturally limits throughput. Additional rate limiting could be added:

```rust
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(2);
let mut last_request_time = Instant::now() - MIN_REQUEST_INTERVAL;

// In the CBOR handling:
if last_request_time.elapsed() < MIN_REQUEST_INTERVAL {
    send_error(device, msg.cid, ERR_CHANNEL_BUSY)?;
    return Ok(());
}
last_request_time = Instant::now();
```

**Severity**: Low (annoying but not security-critical).

#### T6: Virtual Device Persistence After Disconnect

**Threat**: If the CTAP service crashes without cleanup, the virtual FIDO device
remains in `/dev/hidraw*`, potentially confusing the user or other software.

**Mitigation**:
1. The `VirtualFidoDevice` struct implements `Drop` which sends `UHID_DESTROY`
2. The CTAP service's cleanup runs on both normal and error exit paths
3. The connection close handler (`on_close()`) explicitly stops the CTAP service
4. If the RustDesk process is killed (SIGKILL), the kernel automatically destroys
   uhid devices when the fd is closed (the fd is owned by the process)

**Residual risk**: None — kernel handles the worst case.

**Severity**: Low.

#### T7: Side-Channel via Timing

**Threat**: An observer on the network can determine when the user touches their
security key by measuring the time between the CTAP request and response.

**Analysis**: The timing information reveals:
- That a WebAuthn authentication occurred
- How long it took the user to touch their key

This is minimal information leakage. The content of the CTAP2 messages is
encrypted by RustDesk's transport.

**Severity**: Negligible.

## Security Properties Preserved

| Property | Status | Explanation |
|----------|--------|-------------|
| **Private key never leaves authenticator** | Preserved | Hardware-bound keys cannot be extracted regardless of transport |
| **RP origin binding** | Preserved | `rpIdHash` in the CTAP2 request is generated by the remote browser and validated by the authenticator |
| **Replay resistance** | Preserved | Authenticator counter increments on each signature; `clientDataHash` includes a fresh challenge |
| **User presence** | Preserved | Physical touch is required on the local machine |
| **User verification (PIN)** | Preserved | PIN entry happens on the local machine via the key's own PIN interface |
| **Attestation integrity** | Preserved | We forward raw CTAP2 payloads without modification; attestation signatures are from the real key |

## Security Properties Modified

| Property | Change | Assessment |
|----------|--------|------------|
| **Authenticator proximity** | Weakened | RP assumes authenticator is on same machine as browser. In reality, it may be thousands of miles away. |
| **Channel binding** | N/A | CTAP2 does not have channel binding (unlike TLS token binding). No change. |
| **Phishing resistance** | Unchanged | The `rpId` check still works. Phishing risk is inherent to remote desktop. |

## Comparison with Existing Technologies

| Technology | Mechanism | Security Model |
|------------|-----------|----------------|
| **CTAP Hybrid (caBLE)** | Phone authenticator over cloud tunnel | Same as ours — CTAP over an encrypted tunnel. Accepted by FIDO Alliance. |
| **Qubes OS CTAP Proxy** | uhid in browser VM, USB in sys-usb | Same architecture. Widely deployed in security-conscious environments. |
| **Windows Remote Desktop** | WebAuthn redirection via RDP channel | Similar concept but limited to Azure AD / Windows Hello. |
| **RustDesk CTAP Passthrough** | uhid on remote, hidapi on local | Equivalent security properties to CTAP Hybrid. |

#### T8: Windows VHF Driver as Attack Surface

**Threat**: The VHF driver runs in a user-mode driver host process and creates
a virtual HID device accessible to all processes. A malicious local process could
open the driver's device interface and inject CTAP commands or read responses.

**Mitigation**:
1. The driver's device interface uses a security descriptor that restricts access
   to the RustDesk process (matched by process path or a custom security group).
2. The driver only accepts one concurrent client — the first process to open the
   device interface owns it exclusively for that session.
3. The device interface GUID is not publicly documented, reducing discoverability
   (security by obscurity as an additional layer, not the primary control).
4. Same CTAP2 command allowlist applies (defense-in-depth).

**Residual risk**: A local admin or a process running as the same user could
potentially access the driver. This is equivalent to the Linux uhid risk (any
process with write access to `/dev/uhid` can inject events). Mitigated by the
single-client lock.

**Severity**: Low (local privilege required, single-client lock prevents hijacking).

#### T8a: macOS DriverKit Extension as Attack Surface

**Threat**: The DriverKit extension creates a virtual HID device accessible via
IOKit. A local process could open the IOKit user client and inject CTAP commands
or read browser responses.

**Mitigation**:
1. The IOKit user client uses `clientHasPrivilege` checks — only processes signed
   by the same team ID (RustDesk) can open the user client connection.
2. Single concurrent client: the user client rejects a second connection while
   one is active.
3. macOS system extension approval requires explicit user consent — the extension
   cannot be installed silently.
4. The extension runs in a hardened sandbox with minimal privileges (only HID
   device creation via DriverKit).

**Residual risk**: Equivalent to the Linux uhid / Windows VHF risks — a local
process running as the same user with the right code signature could access the
device. Mitigated by single-client lock.

**Severity**: Low (system extension approval + code signing + single-client lock).

#### T9: Malicious Web Page Connects to Companion App (Web Client)

**Threat**: A malicious web page running in the user's browser discovers the
companion's localhost WebSocket port and sends crafted CTAP relay requests to
sign assertions for an attacker-controlled relying party.

**Mitigation**:
1. **Origin checking**: The companion validates the `Origin` header on the
   WebSocket upgrade request. Only allowed origins (the RustDesk web client
   domain, localhost) are accepted.
2. **CTAP2 command allowlist**: Even if an attacker bypasses origin checks,
   the companion only forwards allowed CTAP2 commands (0x01, 0x02, 0x04, 0x06,
   0x08, 0x0B). Destructive operations are blocked.
3. **User presence required**: The physical key still requires a touch. The user
   would see an unexpected "Tap your key" prompt, which is a visible signal.
4. **Single session**: The companion accepts only one WebSocket connection at a
   time. If the RustDesk web client is already connected, the attacker's
   connection is rejected.

**Residual risk**: If the user has no active RustDesk session but the companion
is running, and the attacker can spoof the allowed origin, they could prompt the
user to touch their key. The rpId binding in CTAP2 limits what can be signed,
but the user might touch the key reflexively. Rate limiting (no more than one
request per 2 seconds) further reduces this risk.

**Severity**: Medium if origin check is bypassed, Low with all mitigations.

#### T10: Companion App as Persistent Local Attack Surface (Web Client)

**Threat**: The companion app listens on a localhost port, creating a persistent
attack surface on the user's machine.

**Mitigation**:
1. The companion binds to `127.0.0.1` only (not `0.0.0.0`) — not network-accessible
2. The companion should only run when needed, not as a permanent daemon. Ideal:
   launched on demand by the web client and exits after idle timeout (e.g., 5
   minutes with no WebSocket connection).
3. The companion does not store any state, credentials, or configuration beyond
   the allowed-origin list.

**Severity**: Low.

## Recommendations

### Must-have (before merging)

1. Block authenticatorReset (0x07) — **implemented in Component 06**
2. Block authenticatorCredentialManagement (0x0A) — add to filter
3. Block authenticatorBioEnrollment (0x09) — add to filter
4. Block authenticatorConfig (0x0D) — add to filter
5. Feature disabled by default — **implemented in Component 10**
6. Explicit user opt-in required — **implemented via permission toggle**
7. Clean device teardown on disconnect — **implemented via Drop + on_close**

### Should-have (before 1.0 release)

8. Rate limiting on CTAP requests (2-second minimum interval)
9. Log all CTAP operations for audit trail
10. Display rpId in the "Tap your key" prompt (requires CBOR parsing)
11. Option to require user confirmation before each CTAP relay (beyond the key touch)

### Must-have for Windows VHF Driver (Component 04, Part B)

8f. Driver device interface restricted by security descriptor
8g. Single concurrent client lock on driver device interface
8h. Driver must be attestation-signed (not test-signed) for production
8i. Driver must cleanly destroy virtual device when client disconnects or process crashes

### Must-have for macOS DriverKit Extension (Component 04, Part C)

8j. IOKit user client restricted to RustDesk team ID via `clientHasPrivilege`
8k. Single concurrent user client connection
8l. Extension must be notarized and signed with Developer ID
8m. Extension must request only `com.apple.developer.driverkit.family.hid.virtual.device` entitlement (minimal privileges)

### Must-have for Companion App (Component 13)

8a. Origin validation on WebSocket upgrade — reject unknown origins
8b. Localhost-only binding (127.0.0.1, not 0.0.0.0)
8c. Single active WebSocket session limit
8d. CTAP2 command allowlist enforced in companion (defense-in-depth, mirrors remote filter)
8e. Idle timeout — companion exits after 5 minutes with no connection

### Nice-to-have (future)

12. PIN entry UI on the local machine (currently relies on key's built-in PIN)
13. Allowlist of rpIds that can use CTAP passthrough
14. Metrics/telemetry for CTAP usage (number of transactions, success rate)
15. On-demand companion launch — web client triggers companion start via registered protocol handler
