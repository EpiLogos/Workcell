// Workcell's Action field, as the O:I Action registry holds it (`oi act`,
// O-I #1002, `oi.owner-action-field/v1`). Included by main.rs, beside its
// dispatch.
//
// `workcell actions [--json]` lists the Actions; `workcell action run <id>
// <json|->` runs one. An Action is one of Workcell's own routes: this table
// names its id, title, mutation class and the flags the route takes. Running
// one makes the route's words from the input, checked by name, and runs it
// through the same dispatch, in this process, so the document or refusal is
// Workcell's own. The seats (`workcell.seat.*`) are the seat instrument's
// field, not this one; a held seat's landing (`workcell.lane.*`: commit, push,
// land; landing.rs) is Workcell's, and never writes the seat's claim.

const OWNER_ACTION_FIELD: &str = "oi.owner-action-field/v1";

/// One flag of a route: (input name, flag, type, required, what it is).
type RouteFlag = (&'static str, &'static str, &'static str, bool, &'static str);

struct FieldAction {
    id: &'static str,
    title: &'static str,
    route: &'static [&'static str],
    flags: &'static [RouteFlag],
    mutation_class: &'static str,
}

const FIELD: &[FieldAction] = &[
    FieldAction {
        id: "workcell.status",
        title: "This workcell's status",
        route: &["status"],
        flags: &[],
        mutation_class: "read-only",
    },
    FieldAction {
        id: "workcell.discover",
        title: "Discover the providers and affordances here",
        route: &["discover"],
        flags: &[],
        mutation_class: "read-only",
    },
    FieldAction {
        id: "workcell.places",
        title: "The live place census: rooms where sessions run",
        route: &["places"],
        flags: &[],
        mutation_class: "read-only",
    },
    FieldAction {
        id: "workcell.place.request",
        title: "Claim a persistent place for a session",
        route: &["place", "request"],
        flags: &[
            ("name", "--name", "string", true, "the place's name (a slug)"),
            ("provider", "--provider", "string", false, "auto, herdr or tmux (auto when absent)"),
        ],
        mutation_class: "locally-mutating",
    },
    FieldAction {
        id: "workcell.place.release",
        title: "Give a place back",
        route: &["place", "release"],
        flags: &[
            ("place_ref", "--place-ref", "string", true, "the place's ref, from its grant"),
            ("pid", "--pid", "number", true, "the place's process, from its grant"),
            ("start_marker", "--start-marker", "string", true, "the process's start marker (ps lstart), from its grant"),
            ("provider_close", "--provider-close", "boolean", false, "close the provider's room too"),
        ],
        mutation_class: "locally-mutating",
    },
    FieldAction {
        id: "workcell.declare",
        title: "Declare a World's first workcell: a checkout of its project and a free seat",
        route: &["declare"],
        flags: &[
            ("world", "--world", "string", true, "the World's folder, absolute"),
            ("project", "--project", "string", true, "the project: one folder of the World's Work, standing on main"),
        ],
        mutation_class: "locally-mutating",
    },
    FieldAction {
        id: "workcell.lane.commit",
        title: "Commit a seat's work on its lane's branch",
        route: &["lane", "commit"],
        flags: &[
            ("world", "--world", "string", false, "the World's folder, absolute (else CENTRAL_ROOT)"),
            ("workcell", "--workcell", "string", true, "the workcell that holds the seat"),
            ("product", "--product", "string", true, "the seat's project"),
            ("message", "--message", "string", true, "what the commit holds, in words"),
        ],
        mutation_class: "locally-mutating",
    },
    FieldAction {
        id: "workcell.lane.push",
        title: "Push a seat's lane to its project's remote",
        route: &["lane", "push"],
        flags: &[
            ("world", "--world", "string", false, "the World's folder, absolute (else CENTRAL_ROOT)"),
            ("workcell", "--workcell", "string", true, "the workcell that holds the seat"),
            ("product", "--product", "string", true, "the seat's project"),
        ],
        mutation_class: "externally-mutating",
    },
    FieldAction {
        id: "workcell.lane.land",
        title: "Land a seat's lane: a draft merge request, a merge request, or a merge into main",
        route: &["lane", "land"],
        flags: &[
            ("world", "--world", "string", false, "the World's folder, absolute (else CENTRAL_ROOT)"),
            ("workcell", "--workcell", "string", true, "the workcell that holds the seat"),
            ("product", "--product", "string", true, "the seat's project"),
            ("as", "--as", "string", true, "how the work lands: draft, mr or merge"),
        ],
        mutation_class: "externally-mutating",
    },
    FieldAction {
        id: "workcell.instances.list",
        title: "List the harness instances this workcell knows",
        route: &["instances", "list"],
        flags: &[],
        mutation_class: "read-only",
    },
];

fn action_field() -> Value {
    let actions = FIELD
        .iter()
        .map(|action| {
            let inputs = action
                .flags
                .iter()
                .map(|(name, flag, kind, required, description)| {
                    json!({"name": name, "type": kind, "required": required,
                           "description": description, "flag": flag})
                })
                .collect::<Vec<_>>();
            json!({
                "id": action.id,
                "title": action.title,
                "description": format!("workcell {}", action.route.join(" ")),
                "mutation_class": action.mutation_class,
                "inputs": inputs,
                "output": {"type": "workcell-json"},
                "route": action.route,
                "availability": {"available": true, "reason": null},
            })
        })
        .collect::<Vec<_>>();
    json!({"schema": OWNER_ACTION_FIELD, "owner": "workcell", "actions": actions})
}

/// The route's words an Action's input makes: the route, then each flag it
/// names. An input the route does not take, a required one missing, or a
/// value of the wrong shape is refused by name before anything runs.
fn action_route_words(id: &str, input: &Value) -> Result<Vec<String>, String> {
    let action = FIELD
        .iter()
        .find(|action| action.id == id)
        .ok_or_else(|| format!("`{id}` is not in Workcell's Action field; `workcell actions` lists it"))?;
    let given = input
        .as_object()
        .ok_or_else(|| format!("{id} takes one JSON object of named inputs"))?;
    if let Some(name) = given
        .keys()
        .find(|name| !action.flags.iter().any(|(known, ..)| known == &name.as_str()))
    {
        return Err(format!("{id} takes no input `{name}`"));
    }
    let mut words = action.route.iter().map(|word| word.to_string()).collect::<Vec<_>>();
    for (name, flag, kind, required, _) in action.flags {
        match (given.get(*name), *kind) {
            (None | Some(Value::Null), _) if *required => {
                return Err(format!("{id} needs its input `{name}`"))
            }
            (None | Some(Value::Null), _) => {}
            (Some(Value::Bool(true)), "boolean") => words.push((*flag).into()),
            (Some(Value::Bool(false)), "boolean") => {}
            (Some(_), "boolean") => return Err(format!("{id}'s input `{name}` is true or false")),
            (Some(Value::Number(value)), "number") => {
                words.extend([(*flag).to_owned(), value.to_string()])
            }
            (Some(Value::String(value)), "number") if value.parse::<f64>().is_ok() => {
                words.extend([(*flag).to_owned(), value.clone()])
            }
            (Some(_), "number") => return Err(format!("{id}'s input `{name}` is a number")),
            (Some(Value::String(value)), _) => words.extend([(*flag).to_owned(), value.clone()]),
            (Some(_), _) => return Err(format!("{id}'s input `{name}` is a string")),
        }
    }
    Ok(words)
}

fn command_actions(global: &GlobalArgs) -> Result<(), WorkcellError> {
    if global.json {
        emit_json(action_field());
    } else {
        for action in FIELD {
            println!("  {:<28} {:<18} {}", action.id, action.mutation_class, action.title);
        }
    }
    Ok(())
}

/// `workcell action run <id> <json|->`: the Action's route, run through this
/// binary's own dispatch with the same global arguments.
fn command_action_run(global: &GlobalArgs, args: &[String]) -> Result<(), WorkcellError> {
    let (Some("run"), Some(id)) = (args.first().map(String::as_str), args.get(1)) else {
        return Err(WorkcellError::InvalidDemand(
            "usage: workcell action run <id> <json|->".into(),
        ));
    };
    let raw = match args.get(2).map(String::as_str) {
        None => "{}".to_owned(),
        Some("-") => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|error| {
                WorkcellError::InvalidDemand(format!("{id}'s input could not be read: {error}"))
            })?;
            text
        }
        Some(raw) => raw.to_owned(),
    };
    let input = if raw.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str::<Value>(&raw).map_err(|error| {
            WorkcellError::InvalidDemand(format!("{id} takes one JSON object: {error}"))
        })?
    };
    let words = action_route_words(id, &input).map_err(WorkcellError::InvalidDemand)?;
    let mut routed = global.clone();
    routed.json = true;
    routed.remaining = words;
    dispatch(&routed)
}

#[cfg(test)]
mod action_field_tests {
    use super::*;

    #[test]
    fn the_field_lists_workcells_actions_under_its_prefix() {
        let field = action_field();
        assert_eq!(field["schema"], OWNER_ACTION_FIELD);
        assert_eq!(field["owner"], "workcell");
        for action in field["actions"].as_array().unwrap() {
            let id = action["id"].as_str().unwrap();
            assert!(id.starts_with("workcell.") && !id.starts_with("workcell.seat."), "{id}");
        }
        let request = field["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["id"] == "workcell.place.request")
            .unwrap();
        assert_eq!(request["inputs"][0]["name"], "name");
        assert_eq!(request["inputs"][0]["required"], true);
    }

    #[test]
    fn an_input_makes_the_routes_words_and_is_refused_by_name() {
        assert_eq!(
            action_route_words(
                "workcell.place.release",
                &json!({"place_ref": "place:1", "pid": 42, "start_marker": "Thu Nov 13", "provider_close": true})
            )
            .unwrap(),
            ["place", "release", "--place-ref", "place:1", "--pid", "42", "--start-marker", "Thu Nov 13", "--provider-close"]
        );
        assert_eq!(
            action_route_words("workcell.place.request", &json!({"name": "lane"})).unwrap(),
            ["place", "request", "--name", "lane"]
        );
        for (id, input, words) in [
            ("workcell.place.request", json!({"name": "lane", "colour": "red"}), "takes no input `colour`"),
            ("workcell.place.request", json!({}), "needs its input `name`"),
            ("workcell.place.release", json!({"place_ref": "p", "pid": "x", "start_marker": "m"}), "is a number"),
            ("workcell.place.release", json!({"place_ref": "p", "pid": 1, "start_marker": "m", "provider_close": "yes"}), "true or false"),
            ("workcell.status", json!([]), "one JSON object"),
            ("workcell.teleport", json!({}), "is not in Workcell's Action field"),
        ] {
            let refused = action_route_words(id, &input).unwrap_err();
            assert!(refused.contains(words), "{id}: {refused}");
        }
    }

    #[test]
    fn every_route_in_the_field_is_one_workcell_dispatches() {
        // A state root of the test's own: the read-only routes may run; a
        // writing route's unknown flag is refused before any effect.
        let root = std::env::temp_dir().join(format!("workcell-action-field-{}", std::process::id()));
        for action in FIELD {
            let mut words = vec!["--state-root".to_owned(), root.to_string_lossy().into_owned()];
            words.extend(action.route.iter().map(|word| word.to_string()));
            words.push("--no-such-flag-for-this-check".into());
            let global = parse_global(words).unwrap();
            if let Err(error) = dispatch(&global) {
                let said = error.to_string();
                assert!(
                    !said.contains("unknown command") && !said.contains("unknown place subcommand"),
                    "{} names a route Workcell does not have: {said}",
                    action.id
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
