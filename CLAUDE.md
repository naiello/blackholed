# blackholed

`blackholed` is a DNS resolver that provides the following features:
1. Block DNS requests (return NXDOMAIN) for domains on a blocklist
2. Automatically update blocklists from disk or from a URL
3. Provide a web UI and management API to allow easy self-service unblocking of domains

## Resolver

The resolver layer uses hickory_server to listen for TCP and UDP connections. It will first check the BlocklistAuthority to see if the domain is blocklisted.
If the domain is blocklisted, the BlocklistAuthority returns NXDOMAIN and the response is returned to the requester. If the domain is not blocked, ForwardAuthority
is used to forward the request to an upstream server using DNS-over-TLS (DOT).

### BlocklistAuthority

This is a custom implementation of a blocklist resolver. It will check requested domains against an in-memory table, including wildcard matches. It will use the most specific match found in the table
in the case of multiple matching wildcard entries. Entries in the table can either have a disposition of Block or Allow.

If a domain is blocked, this authority will emit a message over a tokio broadcast channel to notify other components of the event (for eventual storage in Redis), including the 
request source address, domain, record type, and timestamp.

## Updater (not implemented)

The updater is a component that runs periodically to identify sources that have either file paths or URLs. The updater will periodically execute the following sequence:
1. Check the SQL database for any sources that have URLs or file paths defined that have not been updated in more than a week
2. Downloads (for URLs) or loads from disk (for file paths) the new version of the source
3. Parses the source list
4. Loads the hosts into the SQL database
5. Finds all hosts in the SQL database where the updated_at timestamp is older than the time we started this refresh run and deletes them
6. Triggers the BlocklistAuthority to reload its list of blocked and allowed hosts

When a blocklist is loaded, each line is parsed according to the following rules:
```
# this is a comment
0.0.0.0 blockme.com # This is also a comment. Ignore sinkhole IPs like 0.0.0.0 or 127.0.0.1, use the second field only.
blockme.com         # If the domain is by itself on a line, use it. Should be normalized to `blockme.com.`.
*.blockme.com       # Wildcards are allowed, this should be normalized to `*.blockme.com.`
*.com               # This is NOT allowed and should be dropped (wildcards must be at least 2 layers deep)
```

A domain may or may not include a trailing `.`. If there is no trailing `.`, one should be added automatically.

If loading from a file or URL, all entries must have the same disposition (Block or Allow). For manually-managed sources, Block/Allow can be mix/matched.

## Web Components (not implemented)

The web components are implemented using Axum, HTMX, and TailwindCSS. It listens on port 5355 by default. The UI provides a minimal and clean interface for users to interact with
their `blackholed` instance.

### Management API (not implemented)

The management API is available under the `/api` route and allows the following tasks:
1. Create, read, delete, list all blocklist/allowlist sources (backed by the SQL database)
2. For sources (blocklists/allowlists) that are manually managed (have no URL or filename defined), upserting or deleting host entires from the source. (backed by the SQL database)
    - For domains that the user request unblocking through the UI, an allowlist entry is made
3. List recent block events by IP address (backed by Valkey)
4. List all active clients (backed by Valkey)

All APIs that return lists should be paginated to 100 items.

### Web UI (not implemented)

The web UI is a self-service way for clients of `blackholed` to manage the behavior of the daemon. It must be resolvable by the Resolver layer at `blackhole.local`.

When a client loads the web UI, by default, they are presented with a list of recent blocked domains for their IP address, sorted by recency (latest first).
Each blocked domain should have a button that allows the domain to be permanently allowlisted (this works by adding an allowlist entry to the `webmanaged` source via the management API).
While this page defaults to the user's IP address, it provides a text box for the user to enter a different IP address and see the results for that IP address.
There is also a page that lists all active clients of the DNS server by IP and allows the user to click a link and see recent blocked events for that IP.

The web UI also allows for management of sources. Users can do the following:
1. Add a new manually-managed source, and add/delete/list hosts from this source.
2. Add a new automatically-managed source given a URL and/or a file path on disk.

## Persistence

### SQL Database

Implementation: `src/db.rs`
Model: `src/model.rs`

The SQL database is used to persist durable data. It has the following tables:
- `source` - contains the sources of blocked or allowed hosts. This may contain a URL or a file path on the local disk, which will be used for automatic updates. If no URL or file path is present, this is a manually-updated source that cannot be automatically refreshed
- `host` - contains entries from the various blocklists and allowlists from the `source` table along with their last update timestamps, in order to facilitate aging out old data after an update

The implementation uses the `sqlx` library for database access.

### Valkey (not implemented)

The valkey instance is used to persist ephemeral, high-volume data. 

The following keys are used:
- `blocked-${IP}` - contains the blocked domains for each IP address stored in a Sorted Set (where the score is the timestamp that the event occurred).
- `clients` - contains the IP addresses of all clients maintained in the `blocked-${IP}` keys sorted as a Sorted Set (where the score is the timestamp of the last blocked event for that IP)

Every 15 minutes, the following cleanup process should run:
- Read the `clients` key for all active IPs, then for each `blocked-${IP}` key identified, use `ZREMRANGEBYSCORE` to delete entries older than 1 hour
- Use `ZREMRANGEBYSCORE` on the `clients` key to delete all IPs older than 4 hours

The instance should have a background task listening for events on the `blocked` channel from the BlocklistAuthority, and when a message is received:
- Use `ZADD` to upsert the client IP into the `clients` key
- Use `ZADD` to upsert the domain block event into the `blocked-${IP}` key

The interface should also provide the following features to the management API:
1. List block events for an IP
2. List all active clients

