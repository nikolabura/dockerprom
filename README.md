# dockerprom

This is a **simple, lightweight** Prometheus exporter for Docker container metrics.

- Simple?
    - No Docker socket access or capabilities are required (no `--privileged`). The program will read from the cgroupfs (`/sys/fs/cgroup/` by default) to read metrics information, and the Docker containers directory (`/var/lib/docker/containers/` by default) to add container metadata (name, labels, etc.).
    - Only five metrics are currently exported per container: Memory usage, user CPU time, system CPU time, I/O read bytes, and I/O written bytes.

- Lightweight?
    - In my testing on a machine with seven containers, it uses 500 KiB (**~0.5 MiB**) of memory, **~1% CPU** when queried, and ~0% CPU when idle. In comparison, cadvisor uses 23 MiB of memory (46x more!) and idles at ~4% CPU, even when it's not being actively queried. \
    I haven't tested too thoroughly, so please let me know if it doesn't work as well for you.
    - Pulls metrics from the cgroupfs directly instead of going through `docker stats`, which seems to be quite slow. This is little more than a glorified file exporter.

See the included `docker-compose.yaml` for an example of how it can be deployed as a container. Or go even simpler:

```yaml
services:
  dockerprom:
    image: ghcr.io/nikolabura/dockerprom:master
    ports:
      - 9376:3000
    volumes:
      - /var/lib/docker/containers/:/var/lib/docker/containers/:ro
      - /sys/fs/cgroup:/sys/fs/cgroup:ro
```

If you're running it as a standalone binary (which you can!) note that it listens only on localhost by default. You'll need to supply a listen address, like `-l [::]:9376`, to listen on all interfaces on port 9376, for example.

### Features

- HTTP Basic auth (via argument or environment variable).
- Configuring (blacklist or whitelist) which container labels get transcribed to Prometheus labels.
- Supports both cgroup v1 and v2, and both Docker cgroup drivers (cgroupfs and systemd). Will attempt to autodetect which is in use.

Don't expect this tool to be perfect. Use cadvisor if you need something more battle-tested and with a lot more metrics. This is for those of us who just want a simple, barebones listing of CPU, RAM, and I/O per container.

## Running

### As a standalone binary

Download a binary from the releases page. (This tool doesn't really attempt to support anything other than Linux at the moment, apologies.) These links *should* work:

- [I have an ARM CPU](https://github.com/nikolabura/dockerprom/releases/latest/download/dockerprom-aarch64-unknown-linux-musl.tar.gz)
- [I have an x86 CPU](https://github.com/nikolabura/dockerprom/releases/latest/download/dockerprom-x86_64-unknown-linux-musl.tar.gz)

Extract it and run it with sudo: `sudo ./dockerprom` and you should be able to `curl localhost:3000` to see the metrics. Done!

To expose it to all interfaces, not just localhost, set the `-l` flag. Use whatever port you want. `[::]` will make it listen on both IPv4 and IPv6 interfaces.

```bash
sudo ./dockerprom -l [::]:9376
```

### As a container

An example `docker-compose.yaml` is included.

If you're running it manually, you might use something like:

```bash
docker run --rm --name dockerprom -p 9376:3000 -v /var/lib/docker/containers/:/conts:ro -v /sys/fs/cgroup:/cgfs:ro ghcr.io/nikolabura/dockerprom:master -d /conts -c /cgfs
```

### As a systemd service

```rust
todo!();
```


## Arguments

Most of the arguments can also be provided as environment variables. Use `--help` to get help from the program. All arguments are optional.

`-l` / `--listen-addr`: The address and port the HTTP server will bind to. It will respond to HTTP requests on any URL, no `/metrics` needed.  
For example:  
`-l 127.0.0.1:3000` listens only on localhost on port 3000.  
`-l 0.0.0.0:9376` listens on all IPv4 interfaces on port 9376.  
`-l [::]:9376` listens on all interfaces, IPv4 and IPv6, on port 9376.  
You can also use environment variable `LISTEN_ADDR`, like `LISTEN_ADDR=[::]:9376`

`-d` / `--containers-dir`: The path to the `/var/lib/docker/containers/` directory. Useful if you're running this program in a container and you've bind-mounted it somewhere else.

`-c` / `--cgroupfs-dir`: The path to the `/sys/fs/cgroup/` directory. Same idea as above.

`-B` / `--basicauth`: Basicauth credentials to secure the HTTP server a bit. Supply as username and password with a colon in between. For example: `-B user:pass`  
You can also use environment variable `BASICAUTH`, like `BASICAUTH=user:pass`

`--min-metadata-refresh-ms`: When you query the server and it sees a container ID in the cgroupfs that it doesn't recognize, it'll re-read all the `config.v2.json` files under the `--containers-dir`. This rereading is rate-limited to no more frequent than every 2000 ms by default, but you can change or get rid of this limit.

`--cgroup-version` and `--docker-cgroup-driver`: Use these to override the program's guesses for those values. You can use `docker info | grep Cgroup` to get the Real Answers.

`--exclude-labels`: By default, all the Prometheus metrics will be labeled with the labels of the container, in cadvisor fashion. (The `com.docker.compose.depends_on` label will become `com_docker_compose_depends_on`.) Pass a comma-separated list of container labels here to ignore them when labeling metrics. Make sure you use Docker format (dot-separated), not underscore-separated. For example:  
`--exclude-labels com.docker.compose.depends_on,com.docker.compose.version`

`--include-labels`: Same concept as above, but a whitelist instead of a blacklist. *Only* the comma-separated container labels here will be transfered to metric labels.


## Discussion

### Rationale / Why write this tool?

Because it was fun!

I guess everyone seems to recommend cadvisor for this use-case, but it seems a little bloated (see aforementioned CPU and memory usage) and I'm not really a fan of giving tools flags like `--privileged` if I don't need to. I'm sure there are ways to tune it to fix both of these concerns, but after some searching I couldn't really find 'em.

Also, cadvisor outputs a *ton* of metrics, even after removing a bunch of collectors. I'm sure this is useful for the Google SRE Professionals™ who are smart enough to make use of them all, but that's not me. Sure, you can filter them out on the Prometheus end, but that feels a bit roundabout.

There are plenty of other Docker Prometheus exporters (just Ctrl+F for "docker" [here](https://github.com/prometheus/prometheus/wiki/Default-port-allocations)), but it seems like all of them are either pretty old, or use the `docker stats` API (which in my experience is a bit slower and more CPU-intensive than I'd like it to be, and requires giving the monitoring tool Docker socket access).

It's surprising that there aren't many (... *any* other than cadvisor?) which just use cgroupfs directly, especially when it seems like that's what Docker outright [tells you to do](https://docs.docker.com/config/containers/runmetrics/#control-groups). I'm sure there's a reason for it, or there's one I didn't find, but hey, yeah, it was a fun weekend project to make this one. Hope someone finds it useful!
