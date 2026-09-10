use api_client::{Breakdown, Client, Comment, CreateComment, CreateTicket, CreateWorker, ListTickets, MoveTicket, ReportUsage, Totals, UpdateTicket};
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let token = cli.token.ok_or_else(|| anyhow::anyhow!("set FACTORY_TOKEN or pass --token"))?;
    let client = Client::new(cli.url, token);
    match cli.command {
        Command::Ticket { command } => match command {
            TicketCommand::Create { title, description } => {
                print(&client.create_ticket(&CreateTicket { title, description }).await?)
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
            TicketCommand::Comment { id, body } => print(&client.add_comment(id, &CreateComment { body }).await?),
            TicketCommand::Comments { id } => client.list_comments(id).await?.iter().for_each(print_comment),
            TicketCommand::Resolve { id, cid } => print(&client.resolve_comment(id, cid).await?),
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

fn print<T: Serialize>(t: &T) {
    println!("{}", serde_json::to_string_pretty(t).unwrap());
}

fn print_totals(kind: &str, key: &str, t: &Totals) {
    println!("{kind}\t{key}\t{}\t{}\t{:.4}\t{}\t{}", t.tokens_in, t.tokens_out, t.cost, t.tickets_completed, t.tickets_failed);
}

fn print_comment(c: &Comment) {
    println!("#{}\t{}\t{}\t{}{}", c.id, c.created_at, c.author, if c.resolved { "[resolved] " } else { "" }, c.body);
}
