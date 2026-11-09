//! the `shikumi.evaluation` contract: what one evaluation of the declaration layer's `produce.nix`
//! answers, and the requests that ask for it. the producer plans a composition with one evaluation
//! and evaluates its projects in others, so each answer holds part of what the
//! `shikumi.declarations` document assembles: the manifest and each listed project's declaration
//! file, or some projects' declarations in the order they were asked for.

use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema};
use serde::{Deserialize, Serialize};

use crate::contract::{RelativePath, contract_name, contract_version, require_every_property};
use crate::declarations::{Declaration, Manifest};

contract_name!(EvaluationName, "shikumi.evaluation");

contract_version!(
    /// an evaluation answer's version.
    EvaluationVersion,
    1,
    0
);

/// one evaluation's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "shikumi.evaluation")]
pub struct Evaluation {
    schema: EvaluationName,
    version: EvaluationVersion,
    pub answer: Answer,
}

impl Evaluation {
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Answer {
    Plan(Plan),
    /// each asked project's declaration, in the order asked.
    Projects(Vec<Declaration>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_every_property)]
pub struct Plan {
    pub manifest: Manifest,
    /// every listed project's declaration file, relative to the root, or null where the
    /// manifest's entry names none.
    pub files: BTreeMap<String, Option<RelativePath>>,
}

/// a project to evaluate, by its name and the declaration file the plan gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Project {
    pub name: String,
    pub file: RelativePath,
}

/// what an evaluation is asked for, written as the JSON `produce.nix` reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Request<'a> {
    /// the manifest and its files; no declaration is read.
    Plan {},
    Evaluate(&'a [Project]),
    /// one project, with every string in its declaration written out under a frame that names
    /// its field.
    Locate(&'a Project),
}

impl Request<'_> {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a request always serializes")
    }

    /// the request's name: the one key `produce.nix` reads it by.
    pub fn name(&self) -> String {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(request)) => {
                request.into_iter().next().map(|(name, _)| name)
            }
            _ => None,
        }
        .expect("a request is written as an object with one key")
    }

    /// whether an answer is the one this request asks for. every request and every answer is
    /// named, so a new one of either doesn't build until it's placed here.
    pub fn answered_by(&self, answer: &Answer) -> bool {
        match (self, answer) {
            (Self::Plan {}, Answer::Plan(_)) => true,
            (Self::Evaluate(asked), Answer::Projects(projects)) => projects.len() == asked.len(),
            (Self::Locate(_), Answer::Projects(projects)) => projects.len() == 1,
            (Self::Plan {}, Answer::Projects(_))
            | (Self::Evaluate(_) | Self::Locate(_), Answer::Plan(_)) => false,
        }
    }
}

/// the published JSON schema of an evaluation's answer.
pub fn schema() -> Schema {
    schemars::schema_for!(Evaluation)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::contract::render_json;

    #[test]
    fn tracked_schema_matches_the_types() {
        let tracked = include_str!("../schema/evaluation.schema.json");
        assert!(
            tracked == render_json(&schema()),
            "crates/registry/schema/evaluation.schema.json is stale; regenerate it with `shikumi registry schema evaluation`"
        );
    }

    #[test]
    fn requests_are_written_as_produce_nix_reads_them() {
        let project = |name: &str| Project {
            name: name.to_owned(),
            file: RelativePath::new(format!("{name}/project.nix")).unwrap(),
        };
        let names = [project("a"), project("b")];
        assert_eq!(Request::Plan {}.to_json(), r#"{"plan":{}}"#);
        assert_eq!(
            Request::Evaluate(&names).to_json(),
            r#"{"evaluate":[{"name":"a","file":"a/project.nix"},{"name":"b","file":"b/project.nix"}]}"#
        );
        assert_eq!(
            Request::Locate(&names[0]).to_json(),
            r#"{"locate":{"name":"a","file":"a/project.nix"}}"#
        );
    }

    // `nix/declarations-no-builds.sh` asks the layer for a planted declaration without the
    // producer, with the request tracked here; this holds it to the producer's own.
    #[test]
    fn the_no_builds_check_asks_for_a_planted_project_as_the_producer_would() {
        let tracked = include_str!("../fixtures/planted-request.json");
        let app = [Project {
            name: "app".to_owned(),
            file: RelativePath::new(format!("app/{}", crate::layer::DECLARATION.name)).unwrap(),
        }];
        assert_eq!(tracked.trim_end(), Request::Evaluate(&app).to_json());
    }

    #[test]
    fn an_answer_of_another_shape_is_not_the_answer_to_a_request() {
        let names = [Project {
            name: "a".to_owned(),
            file: RelativePath::new("a/project.nix").unwrap(),
        }];
        let projects = |count| {
            Answer::Projects(vec![
                Declaration {
                    factory: true,
                    value: crate::declarations::Node::Null
                };
                count
            ])
        };
        let plan = Answer::Plan(Plan {
            manifest: Manifest {
                file: RelativePath::new("shikumi.nix").unwrap(),
                value: crate::declarations::Node::Null,
            },
            files: BTreeMap::new(),
        });
        assert!(Request::Plan {}.answered_by(&plan));
        assert!(!Request::Plan {}.answered_by(&projects(1)));
        assert!(Request::Evaluate(&names).answered_by(&projects(1)));
        assert!(!Request::Evaluate(&names).answered_by(&projects(2)));
        assert!(!Request::Evaluate(&names).answered_by(&plan));
        assert!(Request::Locate(&names[0]).answered_by(&projects(1)));
        assert!(!Request::Locate(&names[0]).answered_by(&projects(0)));
        assert_eq!(Request::Locate(&names[0]).name(), "locate");
    }

    #[test]
    fn an_answer_reads_only_in_its_contract() {
        let read = |value: serde_json::Value| Evaluation::from_json(value.to_string().as_bytes());
        let projects = json!({
            "schema": "shikumi.evaluation",
            "version": {"major": 1, "minor": 0},
            "answer": {"projects": [{"factory": false, "value": {"kind": "null"}}]}
        });
        assert!(read(projects.clone()).is_ok());
        let mut extra = projects.clone();
        extra["selected"] = json!([]);
        assert!(read(extra).is_err());
        let mut both = projects.clone();
        both["answer"]["plan"] =
            json!({"manifest": {"file": "shikumi.nix", "value": {"kind": "null"}}, "files": {}});
        assert!(read(both).is_err());
        let mut extra = projects.clone();
        extra["answer"]["other"] = json!([]);
        assert!(read(extra).is_err());
        let mut renamed = projects;
        renamed["schema"] = json!("shikumi.declarations");
        assert!(read(renamed).is_err());
    }
}
