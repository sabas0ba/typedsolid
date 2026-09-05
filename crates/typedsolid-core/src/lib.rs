//! CADカーネルから独立した意味モデルとpreflight検証。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Box3 {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Box3 {
    pub fn validate(&self) -> Result<(), String> {
        for axis in 0..3 {
            let lo = self.min[axis];
            let hi = self.max[axis];
            if !lo.is_finite() || !hi.is_finite() || lo.abs() > 1e6 || hi.abs() > 1e6 {
                return Err("coordinates must be finite and within ±1,000,000 mm".into());
            }
            if hi - lo < 0.001 {
                return Err("box dimensions must be at least 0.001 mm".into());
            }
        }
        Ok(())
    }

    pub fn minimum_dimension(&self) -> f64 {
        (0..3)
            .map(|i| self.max[i] - self.min[i])
            .fold(f64::INFINITY, f64::min)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Add,
    Cut,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Base,
    Wall,
    Mount,
    Rib,
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feature {
    pub id: String,
    pub role: Role,
    pub operation: Operation,
    pub bounds: Box3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub id: String,
    pub features: Vec<Feature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keepout {
    pub id: String,
    pub bounds: Box3,
    pub clearance_mm: f64,
    pub access: Option<Access>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    PlusZ,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Units {
    #[serde(rename = "mm")]
    Mm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    NotEvaluated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    FeatureThickness,
    ValidSolid,
    SingleSolid,
    KeepoutClearance,
    AccessClearance,
    PartInterference,
    FinalWallThickness,
    SupportFree,
    Strength,
    Thermal,
}

impl Rule {
    pub const ALL: [Self; 10] = [
        Self::FeatureThickness,
        Self::ValidSolid,
        Self::SingleSolid,
        Self::KeepoutClearance,
        Self::AccessClearance,
        Self::PartInterference,
        Self::FinalWallThickness,
        Self::SupportFree,
        Self::Strength,
        Self::Thermal,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub min_feature_mm: f64,
    pub required: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub schema_version: u32,
    pub units: Units,
    pub parts: Vec<Part>,
    pub keepouts: Vec<Keepout>,
    pub policy: Policy,
}

fn identifier(id: &str) -> bool {
    let reserved = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !reserved.contains(&id)
}

impl Model {
    pub fn from_json(json: &str) -> Result<Self, String> {
        if json.len() > 1_000_000 {
            return Err("model exceeds 1 MB limit".into());
        }
        let model: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        model.validate()?;
        Ok(model)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("unsupported schema_version".into());
        }
        if self.parts.is_empty() || self.parts.len() > 100 {
            return Err("model requires 1..100 parts".into());
        }
        if self.keepouts.len() > 100 {
            return Err("at most 100 keepouts are supported".into());
        }
        let thickness = self.policy.min_feature_mm;
        if !thickness.is_finite() || !(0.001..=1000.0).contains(&thickness) {
            return Err("min_feature_mm must be finite and in [0.001, 1000]".into());
        }
        let mut required = BTreeSet::new();
        for rule in &self.policy.required {
            if !required.insert(rule) {
                return Err("duplicate required rule".into());
            }
        }
        let mut part_ids = BTreeSet::new();
        let mut total = 0;
        for part in &self.parts {
            if !identifier(&part.id) || !part_ids.insert(&part.id) {
                return Err(format!("invalid or duplicate part id: {}", part.id));
            }
            if !part.features.iter().any(|f| f.operation == Operation::Add) {
                return Err(format!("{} requires additive geometry", part.id));
            }
            total += part.features.len();
            if total > 1000 {
                return Err("at most 1000 features are supported".into());
            }
            let mut ids = BTreeSet::new();
            for feature in &part.features {
                if !identifier(&feature.id) || !ids.insert(&feature.id) {
                    return Err(format!("invalid or duplicate feature id: {}", feature.id));
                }
                feature
                    .bounds
                    .validate()
                    .map_err(|e| format!("{}/{}: {e}", part.id, feature.id))?;
            }
        }
        let mut ids = BTreeSet::new();
        for keepout in &self.keepouts {
            if !identifier(&keepout.id) || !ids.insert(&keepout.id) {
                return Err(format!("invalid or duplicate keepout id: {}", keepout.id));
            }
            keepout.bounds.validate()?;
            if !keepout.clearance_mm.is_finite() || !(0.0..=1000.0).contains(&keepout.clearance_mm)
            {
                return Err("clearance_mm must be finite and in [0, 1000]".into());
            }
        }
        Ok(())
    }

    pub fn preflight(&self) -> Result<Report, String> {
        self.validate()?;
        let mut report = Report {
            checks: Vec::new(),
            required: self.policy.required.clone(),
        };
        for part in &self.parts {
            for feature in &part.features {
                if feature.operation == Operation::Add {
                    let measured = feature.bounds.minimum_dimension();
                    report.checks.push(Check {
                        rule: Rule::FeatureThickness,
                        status: if measured >= self.policy.min_feature_mm { Status::Pass } else { Status::Fail },
                        target: format!("{}/{}", part.id, feature.id),
                        message: format!("primitive minimum dimension: {measured} mm; required: {} mm; does not measure post-cut wall thickness", self.policy.min_feature_mm),
                    });
                }
            }
        }
        for rule in Rule::ALL
            .into_iter()
            .filter(|r| *r != Rule::FeatureThickness)
        {
            report.checks.push(Check {
                rule,
                status: Status::NotEvaluated,
                target: "model".into(),
                message: "not evaluated by semantic preflight".into(),
            });
        }
        Ok(report)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub rule: Rule,
    pub status: Status,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub checks: Vec<Check>,
    pub required: Vec<Rule>,
}

impl Report {
    /// 未実施の必須検査を成功扱いしない。幾何学的証拠の真正性はbackendの責務。
    pub fn export_allowed(&self) -> bool {
        !self.checks.iter().any(|c| c.status == Status::Fail)
            && self.required.iter().all(|rule| {
                let checks: Vec<_> = self.checks.iter().filter(|c| c.rule == *rule).collect();
                !checks.is_empty() && checks.iter().all(|c| c.status == Status::Pass)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> Model {
        Model {
            schema_version: 1,
            units: Units::Mm,
            parts: vec![Part {
                id: "base".into(),
                features: vec![Feature {
                    id: "plate".into(),
                    role: Role::Base,
                    operation: Operation::Add,
                    bounds: Box3 {
                        min: [0.; 3],
                        max: [10., 10., 2.],
                    },
                }],
            }],
            keepouts: vec![],
            policy: Policy {
                min_feature_mm: 1.2,
                required: vec![Rule::SingleSolid],
            },
        }
    }
    #[test]
    fn json_round_trip() {
        let json = serde_json::to_string(&model()).unwrap();
        assert!(Model::from_json(&json).is_ok());
    }
    #[test]
    fn preflight_is_not_geometry_approval() {
        assert!(!model().preflight().unwrap().export_allowed());
    }
    #[test]
    fn thin_feature_fails() {
        let mut m = model();
        m.parts[0].features[0].bounds.max[2] = 0.4;
        assert_eq!(m.preflight().unwrap().checks[0].status, Status::Fail);
    }
    #[test]
    fn boundary_thickness_passes() {
        let mut m = model();
        m.parts[0].features[0].bounds.max[2] = 1.2;
        assert_eq!(m.preflight().unwrap().checks[0].status, Status::Pass);
    }
    #[test]
    fn invalid_numbers_rejected() {
        for n in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1., 0., 1e7] {
            let mut m = model();
            m.parts[0].features[0].bounds.max[0] = n;
            assert!(m.validate().is_err());
        }
    }
    #[test]
    fn invalid_policy_rejected() {
        for n in [f64::NAN, f64::INFINITY, -1., 0.] {
            let mut m = model();
            m.policy.min_feature_mm = n;
            assert!(m.validate().is_err());
        }
    }
    #[test]
    fn duplicate_ids_rejected() {
        let mut m = model();
        m.parts.push(m.parts[0].clone());
        assert!(m.validate().is_err());
    }
    #[test]
    fn empty_or_cut_only_part_rejected() {
        let mut m = model();
        m.parts[0].features[0].operation = Operation::Cut;
        assert!(m.validate().is_err());
        m.parts.clear();
        assert!(m.validate().is_err());
    }
    #[test]
    fn unsafe_names_rejected() {
        for id in ["../escape", "a/b", "", "CON", "con", "lpt1", "a.b", "a\\b"] {
            assert!(!identifier(id));
        }
        assert!(identifier("board_tray"));
    }
    #[test]
    fn unknown_fields_and_version_rejected() {
        let mut m = model();
        m.schema_version = 2;
        assert!(m.validate().is_err());
        assert!(serde_json::from_str::<Box3>(r#"{"min":[0,0,0],"max":[1,1,1],"typo":1}"#).is_err());
    }
    #[test]
    fn missing_required_and_any_failure_block_export() {
        let mut r = Report {
            required: vec![Rule::SingleSolid],
            checks: vec![],
        };
        assert!(!r.export_allowed());
        r.checks.push(Check {
            rule: Rule::SingleSolid,
            status: Status::Pass,
            target: "x".into(),
            message: "one solid".into(),
        });
        assert!(r.export_allowed());
        r.checks.push(Check {
            rule: Rule::Thermal,
            status: Status::Fail,
            target: "x".into(),
            message: "too hot".into(),
        });
        assert!(!r.export_allowed());
    }
}
