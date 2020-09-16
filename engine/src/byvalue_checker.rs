// Copyright 2020 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::TypeName;
use std::collections::HashMap;
use syn::{ItemStruct, Type};

enum PODState {
    UnsafeToBePOD(String),
    SafeToBePOD,
    IsPOD,
}

struct StructDetails {
    state: PODState,
    dependent_structs: Vec<TypeName>,
}

impl StructDetails {
    fn new(state: PODState) -> Self {
        StructDetails {
            state,
            dependent_structs: Vec::new(),
        }
    }
}

/// Type which is able to check whether it's safe to make a type
/// fully representable by cxx. For instance if it is a struct containing
/// a struct containing a std::string, the answer is no, because that
/// std::string contains a self-referential pointer. Exact logic here
/// is TBD.
pub struct ByValueChecker {
    // Mapping from type name to whether it is safe to be POD
    results: HashMap<TypeName, StructDetails>,
}

impl ByValueChecker {
    pub fn new() -> Self {
        let mut results = HashMap::new();
        results.insert(
            TypeName::new("CxxString"),
            StructDetails::new(PODState::UnsafeToBePOD(
                "std::string has a self-referential pointer.".to_string(),
            )),
        );
        results.insert(
            TypeName::new("UniquePtr"),
            StructDetails::new(PODState::SafeToBePOD),
        );
        results.insert(
            TypeName::new("i32"),
            StructDetails::new(PODState::SafeToBePOD),
        );
        results.insert(
            TypeName::new("i64"),
            StructDetails::new(PODState::SafeToBePOD),
        );
        results.insert(
            TypeName::new("u32"),
            StructDetails::new(PODState::SafeToBePOD),
        );
        results.insert(
            TypeName::new("u64"),
            StructDetails::new(PODState::SafeToBePOD),
        );
        // TODO expand with all primitives, or find a better way.
        ByValueChecker { results }
    }

    pub fn ingest_struct(&mut self, def: &ItemStruct) {
        // For this struct, work out whether it _could_ be safe as a POD.
        let tyname = TypeName::from_ident(&def.ident);
        let mut field_safety_problem = PODState::SafeToBePOD;
        let fieldlist = self.get_field_types(def);
        for ty_id in &fieldlist {
            match self.results.get(ty_id) {
                None => {
                    field_safety_problem = PODState::UnsafeToBePOD(format!(
                        "Type {} could not be POD because its dependent type {} isn't known",
                        tyname, ty_id
                    ));
                    break;
                }
                Some(deets) => match &deets.state {
                    PODState::UnsafeToBePOD(reason) => {
                        let new_reason = format!("Type {} could not be POD because its dependent type {} isn't safe to be POD. Because: {}", tyname, ty_id, reason);
                        field_safety_problem = PODState::UnsafeToBePOD(new_reason);
                        break;
                    }
                    _ => {}
                },
            }
        }
        let mut my_details = StructDetails::new(field_safety_problem);
        my_details.dependent_structs = fieldlist;
        self.results.insert(tyname, my_details);
    }

    pub fn satisfy_requests(&mut self, mut requests: Vec<TypeName>) -> Result<(), String> {
        while !requests.is_empty() {
            let ty_id = requests.remove(requests.len() - 1);
            let deets = self.results.get_mut(&ty_id);
            match deets {
                None => {
                    return Err(format!(
                        "Unable to make {} POD because we never saw a struct definition",
                        ty_id
                    ))
                }
                Some(deets) => match &deets.state {
                    PODState::UnsafeToBePOD(error_msg) => return Err(error_msg.clone()),
                    PODState::IsPOD => {}
                    PODState::SafeToBePOD => {
                        deets.state = PODState::IsPOD;
                        requests.extend_from_slice(&deets.dependent_structs);
                    }
                },
            }
        }
        Ok(())
    }

    pub fn is_pod(&self, ty_id: &TypeName) -> bool {
        match self
            .results
            .get(ty_id)
            .expect("Type not known to byvalue_checker")
        {
            StructDetails {
                state: PODState::IsPOD,
                dependent_structs: _,
            } => true,
            _ => false,
        }
    }

    fn get_field_types(&self, def: &ItemStruct) -> Vec<TypeName> {
        let mut results = Vec::new();
        for f in &def.fields {
            let fty = &f.ty;
            match fty {
                Type::Path(p) => results.push(TypeName::from_type_path(&p)),
                // TODO handle anything else which bindgen might spit out, e.g. arrays?
                _ => {}
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::ByValueChecker;
    use crate::TypeName;
    use syn::{parse_quote, ItemStruct};

    #[test]
    fn test_primitives() {
        let mut bvc = ByValueChecker::new();
        let t: ItemStruct = parse_quote! {
            struct Foo {
                a: i32,
                b: i64,
            }
        };
        let t_id = TypeName::from_ident(&t.ident);
        bvc.ingest_struct(&t);
        bvc.satisfy_requests(vec![t_id.clone()]).unwrap();
        assert!(bvc.is_pod(&t_id));
    }

    #[test]
    fn test_nested_primitives() {
        let mut bvc = ByValueChecker::new();
        let t: ItemStruct = parse_quote! {
            struct Foo {
                a: i32,
                b: i64,
            }
        };
        bvc.ingest_struct(&t);
        let t: ItemStruct = parse_quote! {
            struct Bar {
                a: Foo,
                b: i64,
            }
        };
        let t_id = TypeName::from_ident(&t.ident);
        bvc.ingest_struct(&t);
        bvc.satisfy_requests(vec![t_id.clone()]).unwrap();
        assert!(bvc.is_pod(&t_id));
    }

    #[test]
    fn test_with_up() {
        let mut bvc = ByValueChecker::new();
        let t: ItemStruct = parse_quote! {
            struct Bar {
                a: UniquePtr<CxxString>,
                b: i64,
            }
        };
        let t_id = TypeName::from_ident(&t.ident);
        bvc.ingest_struct(&t);
        bvc.satisfy_requests(vec![t_id.clone()]).unwrap();
        assert!(bvc.is_pod(&t_id));
    }

    #[test]
    fn test_with_cxxstring() {
        let mut bvc = ByValueChecker::new();
        let t: ItemStruct = parse_quote! {
            struct Bar {
                a: CxxString,
                b: i64,
            }
        };
        let t_id = TypeName::from_ident(&t.ident);
        bvc.ingest_struct(&t);
        assert!(bvc.satisfy_requests(vec![t_id.clone()]).is_err());
    }
}