//! A deliberately bounded OpenUI subset. No expressions, bindings, URLs or executable tools.
use super::contracts::UiNode;
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug)]
enum Expr {
    Str(String),
    Number(u8),
    Ref(String),
    List(Vec<Expr>),
    Call(String, Vec<Expr>),
}
struct Parser<'a> {
    input: &'a str,
    pos: usize,
    count: usize,
}
impl Parser<'_> {
    fn ws(&mut self) {
        while self
            .input
            .as_bytes()
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn take(&mut self, c: u8) -> bool {
        self.ws();
        if self.input.as_bytes().get(self.pos) == Some(&c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        self.ws();
        let start = self.pos;
        while self
            .input
            .as_bytes()
            .get(self.pos)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            self.pos += 1;
        }
        let name = &self.input[start..self.pos];
        if name.is_empty() || name.len() > 64 || name.as_bytes()[0].is_ascii_digit() {
            return Err("Expected a bounded identifier".into());
        }
        Ok(name.into())
    }
    fn sequence(&mut self, close: u8, depth: usize) -> Result<Vec<Expr>, String> {
        let mut values = Vec::new();
        if self.take(close) {
            return Ok(values);
        }
        loop {
            values.push(self.expr(depth + 1)?);
            if self.take(close) {
                return Ok(values);
            }
            if !self.take(b',') {
                return Err("Expected comma or closing delimiter".into());
            }
        }
    }
    fn expr(&mut self, depth: usize) -> Result<Expr, String> {
        self.count += 1;
        if depth > 24 || self.count > 500 {
            return Err("UI is too complex".into());
        }
        self.ws();
        if self.take(b'[') {
            return Ok(Expr::List(self.sequence(b']', depth)?));
        }
        if self.input.as_bytes().get(self.pos) == Some(&b'"') {
            let start = self.pos;
            self.pos += 1;
            let mut escaped = false;
            while let Some(c) = self.input.as_bytes().get(self.pos).copied() {
                self.pos += 1;
                if c == b'"' && !escaped {
                    let text: String = serde_json::from_str(&self.input[start..self.pos])
                        .map_err(|_| "Invalid string")?;
                    if text.chars().count() > 1000 {
                        return Err("UI label too long".into());
                    }
                    return Ok(Expr::Str(text));
                }
                escaped = c == b'\\' && !escaped;
            }
            return Err("Unterminated string".into());
        }
        if self
            .input
            .as_bytes()
            .get(self.pos)
            .is_some_and(u8::is_ascii_digit)
        {
            let start = self.pos;
            while self
                .input
                .as_bytes()
                .get(self.pos)
                .is_some_and(u8::is_ascii_digit)
            {
                self.pos += 1;
            }
            return self.input[start..self.pos]
                .parse()
                .map(Expr::Number)
                .map_err(|_| "Invalid span".into());
        }
        let name = self.ident()?;
        if self.take(b'(') {
            Ok(Expr::Call(name, self.sequence(b')', depth)?))
        } else {
            Ok(Expr::Ref(name))
        }
    }
}

pub(crate) fn parse(definition: &str) -> Result<UiNode, String> {
    if definition.len() > 65_536 || definition.trim().is_empty() {
        return Err("Invalid UI definition size".into());
    }
    if definition.trim_start().starts_with('{') {
        let node: UiNode =
            serde_json::from_str(definition).map_err(|_| "Invalid semantic UI JSON")?;
        fn bounds(node: &UiNode, depth: usize, count: &mut usize) -> Result<(), String> {
            *count += 1;
            if depth > 12 || *count > 100 || node.args.iter().any(|s| s.chars().count() > 1000) {
                return Err("UI exceeds limits".into());
            }
            if (node.kind == "Cell" && node.children.len() != 1)
                || (!matches!(node.kind.as_str(), "Grid" | "Stack" | "Cell")
                    && !node.children.is_empty())
            {
                return Err("Invalid children".into());
            }
            for child in &node.children {
                bounds(child, depth + 1, count)?;
            }
            Ok(())
        }
        bounds(&node, 0, &mut 0)?;
        fn expression(node: &UiNode) -> Result<Expr, String> {
            let args = match node.kind.as_str() {
                "Grid" | "Stack" | "Cell" => {
                    if !node.args.is_empty() {
                        return Err("Layout arguments are not permitted".into());
                    }
                    let children = node
                        .children
                        .iter()
                        .map(expression)
                        .collect::<Result<Vec<_>, _>>()?;
                    if node.kind == "Cell" {
                        vec![
                            children.into_iter().next().ok_or("Cell needs a child")?,
                            Expr::Number(node.span),
                        ]
                    } else {
                        vec![Expr::List(children)]
                    }
                }
                _ => node.args.iter().cloned().map(Expr::Str).collect(),
            };
            if node.kind != "Cell" && node.span != 12 {
                return Err("Only Cell supports span".into());
            }
            Ok(Expr::Call(node.kind.clone(), args))
        }
        return resolve(
            &expression(&node)?,
            "root",
            &BTreeMap::new(),
            &mut HashSet::new(),
            0,
            &mut 0,
        );
    }
    let mut parser = Parser {
        input: definition,
        pos: 0,
        count: 0,
    };
    let mut statements = BTreeMap::new();
    loop {
        parser.ws();
        if parser.pos == definition.len() {
            break;
        }
        let name = parser.ident()?;
        if !parser.take(b'=') {
            return Err("Expected assignment".into());
        }
        let expr = parser.expr(0)?;
        if statements.insert(name, expr).is_some() {
            return Err("Duplicate statement".into());
        }
        parser.take(b';');
    }
    let root = statements
        .get("root")
        .ok_or("A root assignment is required")?;
    let mut count = 0;
    let mut visited = HashSet::new();
    let node = resolve(root, "root", &statements, &mut visited, 0, &mut count)?;
    if visited.len() + 1 < statements.len() {
        return Err("Unused statements are not allowed".into());
    }
    Ok(node)
}

fn resolve(
    expr: &Expr,
    id: &str,
    defs: &BTreeMap<String, Expr>,
    visited: &mut HashSet<String>,
    depth: usize,
    count: &mut usize,
) -> Result<UiNode, String> {
    if depth > 12 || *count >= 100 {
        return Err("UI depth or node limit exceeded".into());
    }
    if let Expr::Ref(name) = expr {
        if !visited.insert(name.clone()) {
            return Err("Cyclic or reused component reference".into());
        }
        return resolve(
            defs.get(name).ok_or("Unknown reference")?,
            name,
            defs,
            visited,
            depth,
            count,
        );
    }
    *count += 1;
    let Expr::Call(kind, args) = expr else {
        return Err("Expected a component".into());
    };
    let mut node = UiNode {
        id: id.into(),
        kind: kind.clone(),
        args: vec![],
        span: 12,
        children: vec![],
    };
    match kind.as_str() {
        "Grid" | "Stack" => {
            let [Expr::List(children)] = args.as_slice() else {
                return Err("Grid/Stack require a list".into());
            };
            for (index, child) in children.iter().enumerate() {
                node.children.push(resolve(
                    child,
                    &format!("{id}_{index}"),
                    defs,
                    visited,
                    depth + 1,
                    count,
                )?);
            }
        }
        "Cell" => {
            let [child, Expr::Number(span)] = args.as_slice() else {
                return Err("Cell requires a child and span".into());
            };
            if ![3, 4, 6, 8, 12].contains(span) {
                return Err("Invalid Cell span".into());
            }
            node.span = *span;
            node.children.push(resolve(
                child,
                &format!("{id}_0"),
                defs,
                visited,
                depth + 1,
                count,
            )?);
        }
        "Text" | "ModelStatus" | "Actions" | "Metric" | "Status" | "Table" | "Chart" => {
            let expected = match kind.as_str() {
                "Metric" | "Status" | "Chart" => 3,
                "Table" => 2,
                _ => 1,
            };
            if args.len() != expected {
                return Err(format!("{kind} requires {expected} string arguments"));
            }
            for arg in args {
                let Expr::Str(value) = arg else {
                    return Err("Only string properties are permitted".into());
                };
                node.args.push(value.clone());
            }
            if !matches!(kind.as_str(), "Text" | "Actions") {
                super::data::validate_source(&node.args[0])?;
                if matches!(kind.as_str(), "Metric" | "Status") {
                    super::data::validate_field(&node.args[0], &node.args[1])?;
                }
                if kind == "Table" {
                    for col in node.args[1].split(',') {
                        super::data::validate_field(&node.args[0], col.trim())?;
                    }
                }
                if kind == "Chart"
                    && (node.args[0] != "runtime.history"
                        || node.args[1] != "time"
                        || node.args[2] != "count")
                {
                    return Err("Chart supports runtime.history time/count only".into());
                }
            }
            if kind == "Actions" && !["refresh", "cancel_run"].contains(&node.args[0].as_str()) {
                return Err("Unknown action".into());
            }
        }
        _ => return Err(format!("Unknown component: {kind}")),
    }
    Ok(node)
}

pub(crate) fn sources(node: &UiNode) -> Vec<String> {
    let mut all = Vec::new();
    fn visit(node: &UiNode, all: &mut Vec<String>) {
        let source = if matches!(
            node.kind.as_str(),
            "ModelStatus" | "Metric" | "Status" | "Table" | "Chart"
        ) {
            node.args.first().map(String::as_str)
        } else if node.kind == "Actions" && node.args.first().is_some_and(|arg| arg == "cancel_run")
        {
            Some("runtime.runs")
        } else {
            None
        };
        if let Some(source) = source {
            if !all.iter().any(|value| value == source) {
                all.push(source.to_string());
            }
        }
        for child in &node.children {
            visit(child, all);
        }
    }
    visit(node, &mut all);
    all
}
