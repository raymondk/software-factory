use api_client::{Client, CreateTicket, ListTickets, MoveTicket, Ticket, UpdateTicket};
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
        },
    }
    Ok(())
}

fn print(t: &Ticket) {
    println!("{}", serde_json::to_string_pretty(t).unwrap());
}
