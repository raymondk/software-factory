use std::time::Duration;

use api_client::{Breakdown, Client, Comment, CreateComment, CreateRelation, CreateTicket, CreateWorker, ListTickets, LogLine, MoveTicket, ReportUsage, Totals, UpdateTicket};
use serde::Serialize;
use clap::{Parser, Subcommand};

/// CLI for the Software Factory orchestrator.
#[derive(Parser)]
struct Cli {
    /// Orchestrator URL
    #[arg(long, env = "FACTORY_URL", global = true, default_value = "http://localhost:8080")]
    url: String,
    /// Bearer token
    #[arg(long, env = "FACTORY_TOKEN", global = true, hide_env_values = true)]
    token: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work with tickets
    Ticket {
        #[command(subcommand)]
        command: TicketCommand,
    },
    /// Work with workers
    Worker {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    /// Print usage totals and breakdowns per ticket, worker, and worker type
    Metrics,
}

#[derive(Subcommand)]
enum WorkerCommand {
    /// Create a worker record; prints its id and token (human token)
    Create {
        #[arg(long = "type")]
        worker_type: String,
    },
    /// List workers
    List,
    /// Mark a worker alive (worker token)
    Register { id: String },
    /// Heartbeat (worker token)
    Heartbeat { id: String },
    /// Pick up the next available ticket for the worker's type (worker token)
    Poll { id: String },
    /// Print a worker's whole log, including lines outside runs
    Logs {
        id: String,
        /// Keep printing new lines while the worker is alive
        #[arg(short, long)]
        follow: bool,
    },
    /// Report tokens and cost spent on a ticket (worker token)
    Usage {
        id: String,
        #[arg(long)]
        ticket: i64,
        #[arg(long)]
        tokens_in: i64,
        #[arg(long)]
        tokens_out: i64,
        /// Dollars
        #[arg(long)]
        cost: f64,
    },
}

#[derive(Subcommand)]
enum TicketCommand {
    /// Create a ticket
    Create {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Initial state (default todo)
        #[arg(long)]
        state: Option<String>,
    },
    /// List tickets in rank order
    List {
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
    },
    /// Show one ticket
    View { id: i64 },
    /// Edit fields of a ticket; omitted fields are untouched
    Edit {
        id: i64,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// One of: todo, ready, in_progress, in_review, failed, done
        #[arg(long)]
        state: Option<String>,
        #[arg(long, conflicts_with = "clear_assignee")]
        assignee: Option<String>,
        #[arg(long)]
        clear_assignee: bool,
        /// Append a link; repeatable
        #[arg(long = "add-link", value_name = "URL")]
        add_link: Vec<String>,
    },
    /// Move a ticket before or after another
    Move {
        id: i64,
        #[arg(long, value_name = "ID", conflicts_with = "after", required_unless_present = "after")]
        before: Option<i64>,
        #[arg(long, value_name = "ID")]
        after: Option<i64>,
    },
    /// Relate a ticket to another: it depends on the other (worked only once that is in_review or done), or is related to it
    Link {
        id: i64,
        #[arg(long, value_name = "ID", required_unless_present = "related_to")]
        depends_on: Option<i64>,
        #[arg(long, value_name = "ID")]
        related_to: Option<i64>,
    },
    /// Remove a relation added with link
    Unlink {
        id: i64,
        #[arg(long, value_name = "ID", required_unless_present = "related_to")]
        depends_on: Option<i64>,
        #[arg(long, value_name = "ID")]
        related_to: Option<i64>,
    },
    /// Print the log of a ticket's latest run
    Logs {
        id: i64,
        /// A specific run instead of the latest
        #[arg(long, value_name = "N")]
        run: Option<i64>,
        /// Keep printing new lines until the run ends
        #[arg(short, long)]
        follow: bool,
    },
    /// Add a comment to a ticket
    Comment {
        id: i64,
        #[arg(long)]
        body: String,
    },
    /// List the comments on a ticket
    Comments { id: i64 },
    /// Resolve a comment
    Resolve { id: i64, cid: i64 },
    /// Unresolve a comment
    Unresolve { id: i64, cid: i64 },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let token = cli.token.ok_or_else(|| anyhow::anyhow!("set FACTORY_TOKEN or pass --token"))?;
    let client = Client::new(cli.url, token);
    match cli.command {
        Command::Ticket { command } => match command {
            TicketCommand::Create { title, description, state } => {
                print(&client.create_ticket(&CreateTicket { title, description, state }).await?)
            }
            TicketCommand::List { state, assignee } => {
                for t in client.list_tickets(&ListTickets { state, assignee }).await? {
                    println!("#{}\t{}\t{}\t{}", t.id, t.state, t.rank, t.title);
                }
            }
            TicketCommand::View { id } => print(&client.get_ticket(id).await?),
            TicketCommand::Edit { id, title, description, state, assignee, clear_assignee, add_link } => {
                let links = if add_link.is_empty() {
                    None
                } else {
                    let mut links = client.get_ticket(id).await?.links;
                    links.extend(add_link);
                    Some(links)
                };
                let assignee = if clear_assignee { Some(None) } else { assignee.map(Some) };
                print(&client.update_ticket(id, &UpdateTicket { title, description, state, assignee, links }).await?)
            }
            TicketCommand::Move { id, before, after } => print(&client.move_ticket(id, &MoveTicket { before, after }).await?),
            TicketCommand::Link { id, depends_on, related_to } => {
                let mut t = None;
                for (kind, other) in [("depends_on", depends_on), ("related_to", related_to)] {
                    if let Some(other) = other {
                        t = Some(client.add_relation(id, &CreateRelation { r#type: kind.into(), ticket: other }).await?);
                    }
                }
                print(&t)
            }
            TicketCommand::Unlink { id, depends_on, related_to } => {
                let mut t = None;
                for (kind, other) in [("depends_on", depends_on), ("related_to", related_to)] {
                    if let Some(other) = other {
                        t = Some(client.remove_relation(id, kind, other).await?);
                    }
                }
                print(&t)
            }
            TicketCommand::Logs { id, run, follow } => {
                let t = client.get_ticket(id).await?;
                let run = match run {
                    Some(run) => run,
                    None => t.runs.first().map(|r| r.id).ok_or_else(|| anyhow::anyhow!("ticket #{id} has no runs"))?,
                };
                let live = async || Ok(client.get_ticket(id).await?.runs.iter().any(|r| r.id == run && r.ended_at.is_none()));
                tail(async |after| client.run_logs(run, after).await, live, follow).await?
            }
            TicketCommand::Comment { id, body } => print(&client.add_comment(id, &CreateComment { body }).await?),
            TicketCommand::Comments { id } => client.list_comments(id).await?.iter().for_each(print_comment),
            TicketCommand::Resolve { id, cid } => print(&client.resolve_comment(id, cid).await?),
            TicketCommand::Unresolve { id, cid } => print(&client.unresolve_comment(id, cid).await?),
        },
        Command::Worker { command } => match command {
            WorkerCommand::Create { worker_type } => print(&client.create_worker(&CreateWorker { worker_type }).await?),
            WorkerCommand::List => {
                for w in client.list_workers().await? {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        w.id,
                        w.worker_type,
                        w.status,
                        w.last_heartbeat.as_deref().unwrap_or("-"),
                        w.ticket.map(|t| format!("#{t}")).unwrap_or_else(|| "-".into())
                    );
                }
            }
            WorkerCommand::Logs { id, follow } => {
                let live = async || Ok(client.list_workers().await?.iter().any(|w| w.id == id && w.status != "dead"));
                tail(async |after| client.worker_logs(&id, after).await, live, follow).await?
            }
            WorkerCommand::Register { id } => print(&client.register(&id).await?),
            WorkerCommand::Heartbeat { id } => print(&client.heartbeat(&id).await?),
            WorkerCommand::Poll { id } => print(&client.poll(&id, None).await?),
            WorkerCommand::Usage { id, ticket, tokens_in, tokens_out, cost } => {
                print(&client.report_usage(&id, &ReportUsage { ticket_id: ticket, tokens_in, tokens_out, cost }).await?)
            }
        },
        Command::Metrics => {
            let m = client.metrics().await?;
            println!("kind\tkey\ttokens_in\ttokens_out\tcost\tcompleted\tfailed");
            print_totals("totals", "-", &m.totals);
            m.per_ticket.iter().for_each(|b: &Breakdown<_>| print_totals("ticket", &format!("#{}", b.key.ticket_id), &b.totals));
            m.per_worker.iter().for_each(|b| print_totals("worker", &b.key.worker_id, &b.totals));
            m.per_worker_type.iter().for_each(|b| print_totals("type", &b.key.worker_type, &b.totals));
        }
    }
    Ok(())
}

/// Prints all lines page by page, then, with `follow`, polls every second while `live` holds, plus once after.
async fn tail(
    fetch: impl AsyncFn(Option<i64>) -> Result<Vec<LogLine>, api_client::Error>,
    live: impl AsyncFn() -> anyhow::Result<bool>,
    follow: bool,
) -> anyhow::Result<()> {
    let mut after = None;
    let mut alive = follow;
    loop {
        let lines = fetch(after).await?;
        lines.iter().for_each(|l| println!("{}", l.line));
        if let Some(l) = lines.last() {
            after = Some(l.id);
            if lines.len() >= 1000 {
                continue;
            }
        }
        if !alive {
            return Ok(());
        }
        alive = live().await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn print<T: Serialize>(t: &T) {
    println!("{}", serde_json::to_string_pretty(t).unwrap());
}

fn print_totals(kind: &str, key: &str, t: &Totals) {
    println!("{kind}\t{key}\t{}\t{}\t{:.4}\t{}\t{}", t.tokens_in, t.tokens_out, t.cost, t.tickets_completed, t.tickets_failed);
}

fn print_comment(c: &Comment) {
    println!("#{}\t{}\t{}\t{}{}", c.id, c.created_at, c.author, if c.resolved { "[resolved] " } else { "" }, c.body);
}
