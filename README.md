# blackholed

`blackholed` is a utility to generate and update an /etc/hosts-based DNS blocklist.

## Usage

Blackholed can be invoked without any command-line arguments. It is able to be invoked directly
from the command-line, but it is most useful as a cron. For example:

```cron
# Refresh DNS blocklist at 4:05am every Sunday
5 4 * * 0 /usr/local/bin/blackholed
```

## Config

Blackholed can be configured by a config file at `/etc/blackholed/blackholed.toml`.
Here's an example configuration:

```toml
hosts_file="/var/db/blackholed/hosts"  # location to store the generated hosts file
blackhole_address="0.0.0.0"  # sinkhole address

[dnsmasq]
hup_after_refresh=true  # after refreshing, attempt to SIGHUP the running dnsmasq process
pid_file="/var/run/dnsmasq.pid"  # used to locate the running dnsmasq process

# Blocklist sources are added in individual config sections
[blocklists.disconnect-me-ads] 
url="https://s3.amazonaws.com/lists.disconnect.me/simple_ad.txt"

[blocklists.disconnect-me-mal]
url="https://s3.amazonaws.com/lists.disconnect.me/simple_malvertising.txt"

[blocklists.adguard-dns]
url="https://v.firebog.net/hosts/AdguardDNS.txt"

[blocklists.adaway]
url="https://adaway.org/hosts.txt"
```

## Use with dnsmasq

In order to use the blocklist with `dnsmasq`, ensure your system configures the `--addn-hosts`
option to point to the generated blocklist (in `/var/db/blackholed/hosts` by default). The
`dnsmasq.hup_after_refresh` and `dnsmasq.pid_file` config options tell `blackholed` to send
a SIGUP to dnsmasq after a refresh cycle is complete in order to pick up the updates to the hosts
file.

## Permissions

`blackholed` requires the following permissions:

* Read access to `/etc/blackholed`
* Write access to the configured hosts file location (default: `/var/db/blackholed`)
* Permission to send signals (i.e. kill) the dnsmasq process

While `blackholed` can be run as root, it is recommended to run it as the same user that dnsmasq 
runs as.

