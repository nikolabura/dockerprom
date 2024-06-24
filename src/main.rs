mod containers;
mod metrics;
mod cli;

use cli::{Cli, cfg};
use containers::{refresh_containers_map, CONTAINERS_MAP};
use metrics::{get_metrics_string, print_cgroup_detection_results};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use signal_hook::iterator::Signals;

extern crate pretty_env_logger;
#[macro_use] extern crate log;

async fn service(req: Request<Incoming>) -> http::Result<Response<String>> {
    debug!("Got request for {}", req.uri());

    if let Some(req_auth) = &cfg().basicauth_encoded {
        let auth_hdr = req.headers().get("Authorization");
        if auth_hdr.is_none() || auth_hdr.unwrap() != req_auth {
            debug!("Basicauth failed.");
            if auth_hdr.is_none() { trace!("No Authorization header") }
            else { trace!("Got wrong contents: {:?}", auth_hdr.unwrap()) }
            
            return Response::builder()
                .status(401)
                .header("WWW-Authenticate", "Basic")
                .body("".to_owned())
        }
    }

    match get_metrics_string() {
        Ok(output) => Response::builder().body(output),
        Err(err) => {
            error!("Failed getting metrics: {err}");
            Response::builder()
                .status(500)
                .body("Error occured. Please see logs.".to_owned())
        }
    }
}

fn register_terminate_signal() {
    let mut signals = Signals::new(signal_hook::consts::TERM_SIGNALS).unwrap();
    std::thread::spawn(move || {
        let sig = signals.forever().next().unwrap();
        eprintln!();
        error!("Received signal {}, terminating.", match sig {
            15 => "SIGTERM", 3 => "SIGQUIT", 2 => "SIGINT", _ => "?"
        });
        std::process::exit(1);
    });
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::start();
    info!("Starting Docker container metrics Prometheus exporter.");
    debug!("Debug logging is enabled.");
    trace!("Trace logging is enabled.");

    {
        let mut cont_map = CONTAINERS_MAP.lock().unwrap();
        refresh_containers_map(&mut cont_map);
    }

    print_cgroup_detection_results();
    register_terminate_signal();

    let listener = TcpListener::bind(cli.listen_addr).await?;
    info!("Listening on {}...", listener.local_addr()?);

    loop {
        let (stream, _) = listener.accept().await?;
        debug!("New connection from {:?}", stream.peer_addr().unwrap());
        let io = TokioIo::new(stream);

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, service_fn(service))
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}