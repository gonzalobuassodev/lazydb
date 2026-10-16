use secrecy::SecretString;
use uuid::Uuid;

use crate::{
    db::mongodb::principal::MongoPrincipalMutation, model::secret_text_input::SecretTextInput,
    security::RedactedSecret,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoPrincipalKind {
    User,
    Role,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MongoPrincipalMutationDraft {
    pub profile_id: Uuid,
    pub connection_generation: u64,
    pub database: String,
    pub operation: crate::model::text_input::TextInput,
    pub target: crate::model::text_input::TextInput,
    pub roles_json: crate::model::text_input::TextInput,
    pub privileges_json: crate::model::text_input::TextInput,
    password: SecretTextInput,
    pub selected_field: MongoPrincipalField,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MongoPrincipalField {
    Operation,
    Target,
    Password,
    Roles,
    Privileges,
}

impl MongoPrincipalMutationDraft {
    pub fn new(profile_id: Uuid, generation: u64, database: impl Into<String>) -> Self {
        let database = database.into();
        let default_roles = format!(
            "[{{\"role\":\"read\",\"db\":{}}}]",
            serde_json::to_string(&database).unwrap_or_else(|_| "\"test\"".into())
        );
        Self {
            profile_id,
            connection_generation: generation,
            database,
            operation: "createUser".into(),
            target: String::new().into(),
            roles_json: default_roles.into(),
            privileges_json: "[]".into(),
            password: SecretTextInput::default(),
            selected_field: MongoPrincipalField::Operation,
        }
    }

    pub fn new_operation(
        profile_id: Uuid,
        generation: u64,
        database: impl Into<String>,
        operation: &str,
    ) -> Self {
        let mut draft = Self::new(profile_id, generation, database);
        draft.set_operation(operation);
        draft.selected_field = MongoPrincipalField::Operation;
        if matches!(operation, "createRole" | "updateRole") {
            draft.set_roles_json("[]");
            draft.set_privileges_json("[{\"resource\":{\"db\":\"placeholder\",\"collection\":\"\"},\"actions\":[\"find\"]}]");
        }
        if matches!(operation, "grantRolesToUser" | "revokeRolesFromUser") {
            draft.set_roles_json("[{\"role\":\"read\",\"db\":\"placeholder\"}]");
        }
        if matches!(
            operation,
            "grantPrivilegesToRole" | "revokePrivilegesFromRole"
        ) {
            draft.set_roles_json("[]");
            draft.set_privileges_json("[{\"resource\":{\"db\":\"placeholder\",\"collection\":\"\"},\"actions\":[\"find\"]}]");
        }
        if operation == "updateUser" {
            draft.set_roles_json("[]");
        }
        draft
    }

    pub fn set_operation_with_defaults(&mut self, operation: &str) {
        self.set_operation(operation);
        match operation {
            "createUser" => self.set_roles_json(format!(
                "[{{\"role\":\"read\",\"db\":{}}}]",
                serde_json::to_string(&self.database).unwrap_or_else(|_| "\"test\"".into())
            )),
            "updateUser" => self.set_roles_json("[]"),
            "createRole" | "updateRole" | "grantPrivilegesToRole" | "revokePrivilegesFromRole" => {
                self.set_roles_json("[]");
                self.set_privileges_json(format!("[{{\"resource\":{{\"db\":\"{}\",\"collection\":\"\"}},\"actions\":[\"find\"]}}]", self.database));
            }
            "grantRolesToUser" | "revokeRolesFromUser" => self.set_roles_json(format!(
                "[{{\"role\":\"read\",\"db\":{}}}]",
                serde_json::to_string(&self.database).unwrap_or_else(|_| "\"test\"".into())
            )),
            "dropUser" | "dropRole" => {
                self.set_roles_json("[]");
                self.set_privileges_json("[]");
            }
            _ => {}
        }
    }

    pub fn next_field(&mut self) {
        self.selected_field = match self.selected_field {
            MongoPrincipalField::Operation => MongoPrincipalField::Target,
            MongoPrincipalField::Target => MongoPrincipalField::Password,
            MongoPrincipalField::Password => MongoPrincipalField::Roles,
            MongoPrincipalField::Roles => MongoPrincipalField::Privileges,
            MongoPrincipalField::Privileges => MongoPrincipalField::Operation,
        };
    }

    pub fn previous_field(&mut self) {
        self.selected_field = match self.selected_field {
            MongoPrincipalField::Operation => MongoPrincipalField::Privileges,
            MongoPrincipalField::Target => MongoPrincipalField::Operation,
            MongoPrincipalField::Password => MongoPrincipalField::Target,
            MongoPrincipalField::Roles => MongoPrincipalField::Password,
            MongoPrincipalField::Privileges => MongoPrincipalField::Roles,
        };
    }

    pub fn set_password(&mut self, password: impl Into<String>) {
        self.password.set(password);
    }

    pub fn password_value(&self) -> &str {
        self.password.value()
    }

    pub fn password_cursor(&self) -> usize {
        self.password.cursor()
    }

    pub fn selected_field_value(&self) -> &str {
        self.field_value(self.selected_field)
    }

    pub fn selected_field_cursor(&self) -> usize {
        self.field_cursor(self.selected_field)
    }

    pub fn field_value(&self, field: MongoPrincipalField) -> &str {
        match field {
            MongoPrincipalField::Operation => self.operation.value(),
            MongoPrincipalField::Target => self.target.value(),
            MongoPrincipalField::Password => self.password.value(),
            MongoPrincipalField::Roles => self.roles_json.value(),
            MongoPrincipalField::Privileges => self.privileges_json.value(),
        }
    }

    pub fn field_cursor(&self, field: MongoPrincipalField) -> usize {
        match field {
            MongoPrincipalField::Operation => self.operation.cursor(),
            MongoPrincipalField::Target => self.target.cursor(),
            MongoPrincipalField::Password => self.password.cursor(),
            MongoPrincipalField::Roles => self.roles_json.cursor(),
            MongoPrincipalField::Privileges => self.privileges_json.cursor(),
        }
    }

    pub fn move_field_cursor(&mut self, field: MongoPrincipalField, delta: isize) {
        if field == MongoPrincipalField::Password {
            let chars = self.password.value().chars().count();
            let cursor = self.password.cursor() as isize;
            self.password
                .set_cursor((cursor + delta).clamp(0, chars as isize) as usize);
            return;
        }
        let input = match field {
            MongoPrincipalField::Operation => &mut self.operation,
            MongoPrincipalField::Target => &mut self.target,
            MongoPrincipalField::Roles => &mut self.roles_json,
            MongoPrincipalField::Privileges => &mut self.privileges_json,
            MongoPrincipalField::Password => return,
        };
        let cursor = input.cursor() as isize;
        let chars = input.value().chars().count();
        input.set_cursor((cursor + delta).clamp(0, chars as isize) as usize);
    }

    pub fn set_field_cursor(&mut self, field: MongoPrincipalField, cursor: usize) {
        if field == MongoPrincipalField::Password {
            self.password.set_cursor(cursor);
            return;
        }
        match field {
            MongoPrincipalField::Operation => self.operation.set_cursor(cursor),
            MongoPrincipalField::Target => self.target.set_cursor(cursor),
            MongoPrincipalField::Roles => self.roles_json.set_cursor(cursor),
            MongoPrincipalField::Privileges => self.privileges_json.set_cursor(cursor),
            MongoPrincipalField::Password => {}
        }
    }

    pub fn insert_password(&mut self, character: char) {
        self.password.insert(character);
    }

    pub fn paste_password(&mut self, value: &str) {
        self.password.paste(value);
    }

    pub fn backspace_password(&mut self) {
        self.password.backspace();
    }

    pub fn delete_password(&mut self) {
        self.password.delete();
    }

    pub fn move_password_left(&mut self) {
        self.password.move_left();
    }

    pub fn move_password_right(&mut self) {
        self.password.move_right();
    }

    pub fn move_password_home(&mut self) {
        self.password.move_home();
    }

    pub fn move_password_end(&mut self) {
        self.password.move_end();
    }

    pub fn set_password_cursor(&mut self, cursor: usize) {
        self.password.set_cursor(cursor);
    }

    pub fn password(&self) -> Option<SecretString> {
        (!self.password.value().is_empty()).then(|| self.password.secret().clone())
    }

    pub fn set_secret_cursor(&mut self, cursor: usize) {
        self.password.set_cursor(cursor);
    }

    pub fn insert_secret(&mut self, character: char) {
        self.password.insert(character);
    }

    pub fn paste_secret(&mut self, text: &str) {
        self.password.paste(text);
    }

    pub fn secret_home(&mut self) {
        self.password.move_home();
    }

    pub fn secret_end(&mut self) {
        self.password.move_end();
    }

    pub fn password_redacted(&self) -> Option<RedactedSecret> {
        (!self.password.value().is_empty()).then(|| RedactedSecret::new(self.password.value()))
    }

    pub fn display_target(&self) -> &str {
        self.target.value()
    }

    pub fn display_operation(&self) -> &str {
        self.operation.value()
    }

    pub fn set_target(&mut self, value: impl Into<String>) {
        self.target.set(value);
    }

    pub fn set_operation(&mut self, value: impl Into<String>) {
        self.operation.set(value);
    }

    pub fn set_roles_json(&mut self, value: impl Into<String>) {
        self.roles_json.set(value);
    }

    pub fn set_privileges_json(&mut self, value: impl Into<String>) {
        self.privileges_json.set(value);
    }

    pub fn roles_json_value(&self) -> &str {
        self.roles_json.value()
    }

    pub fn privileges_json_value(&self) -> &str {
        self.privileges_json.value()
    }

    pub fn exact_target_confirmation(&self) -> String {
        format!("{}:{}", self.database, self.target.value())
    }

    pub fn confirmation_target(&self) -> String {
        self.validate()
            .map(|mutation| mutation.confirmation_target())
            .unwrap_or_else(|_| self.exact_target_confirmation())
    }

    pub fn is_destructive(&self) -> bool {
        matches!(self.operation.value(), "dropUser" | "dropRole")
    }

    pub fn validate(&self) -> Result<MongoPrincipalMutation, String> {
        let roles = parse_documents(self.roles_json.value(), "roles")?;
        let privileges = parse_documents(self.privileges_json.value(), "privileges")?;
        MongoPrincipalMutation::new(
            self.database.clone(),
            self.operation.value(),
            self.target.value().to_owned(),
            self.password(),
            roles,
            privileges,
        )
        .map_err(str::to_owned)
    }
}

fn parse_documents(source: &str, field: &str) -> Result<Vec<mongodb::bson::Document>, String> {
    let values: Vec<serde_json::Value> =
        serde_json::from_str(source).map_err(|_| format!("{field} must be a JSON array"))?;
    values
        .into_iter()
        .map(|value| {
            mongodb::bson::Bson::try_from(value)
                .map_err(|_| format!("{field} contains invalid BSON JSON"))?
                .as_document()
                .cloned()
                .ok_or_else(|| format!("each {field} entry must be an object"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_is_ephemeral_and_target_confirmation_is_exact() {
        let mut draft = MongoPrincipalMutationDraft::new(Uuid::new_v4(), 4, "app");
        draft.target.set("reader");
        draft.set_password("secret-value");
        assert_eq!(draft.exact_target_confirmation(), "app:reader");
        assert!(!format!("{draft:?}").contains("secret-value"));
        assert!(draft.validate().is_ok());
    }

    #[test]
    fn create_role_requires_typed_privileges_and_admin_scope() {
        let mut draft = MongoPrincipalMutationDraft::new(Uuid::new_v4(), 1, "admin");
        draft.operation.set("createRole");
        draft.target.set("report-reader");
        draft.roles_json.set("[]");
        draft
            .privileges_json
            .set(r#"[{"resource":{"db":"app","collection":""},"actions":["find"]}]"#);
        assert!(draft.validate().is_ok());
        draft.database = "app".into();
        assert!(draft.validate().is_err());
    }
}
