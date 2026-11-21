//! Core, application and custom document properties.

use super::*;

impl Document {
    // ---- Metadata access ----

    /// Return the complete core-properties model.
    pub fn core_properties(&self) -> Option<&CoreProperties> {
        self.core_properties.as_deref()
    }

    /// Replace the complete core-properties model.
    ///
    /// A value holding a character XML 1.0 cannot carry is refused, naming the
    /// property, and the document is left unchanged.
    pub fn set_core_properties(&mut self, properties: CoreProperties) -> Result<()> {
        for (name, value) in [
            ("title", &properties.title),
            ("creator", &properties.creator),
            ("subject", &properties.subject),
            ("description", &properties.description),
            ("keywords", &properties.keywords),
            ("last modified by", &properties.last_modified_by),
            ("created", &properties.created),
            ("modified", &properties.modified),
            ("category", &properties.category),
            ("content status", &properties.content_status),
            ("identifier", &properties.identifier),
            ("language", &properties.language),
            ("last printed", &properties.last_printed),
            ("revision", &properties.revision),
            ("version", &properties.version),
        ] {
            if let Some(value) = value {
                oxoffice_core::xml::reject_non_xml_characters(
                    &format!("core property {name}"),
                    value,
                )?;
            }
        }
        self.edit_properties(|document| {
            document.reserve_core_properties_bundle()?;
            document.core_properties = Some(Box::new(properties.clone()));
            Ok(())
        })
    }

    /// Remove the complete core-properties part and its package relationship.
    pub fn remove_core_properties(&mut self) -> Result<Option<CoreProperties>> {
        let Some(properties) = self.core_properties.as_deref().cloned() else {
            return Ok(None);
        };
        let mut candidate = self.clone_for_staging();
        if let Some(part_name) = candidate.core_properties_part_name.take() {
            candidate.remove_package_properties_bundle(&part_name, rel_types::CORE_PROPERTIES);
        }
        candidate.core_properties = None;
        self.commit_staged_mutation(candidate);
        Ok(Some(properties))
    }

    /// Return the complete application-properties model.
    pub fn application_properties(&self) -> Option<&AppProperties> {
        self.application_properties.as_ref()
    }

    /// Replace the complete application-properties model.
    ///
    /// An `application_version` must have the form `XX.YYYY` (ECMA-376 Part 1,
    /// 22.2.2.3), because Word refuses to open a package with any other shape.
    pub fn set_application_properties(&mut self, properties: AppProperties) -> Result<()> {
        for (name, value) in [
            ("template", &properties.template),
            ("manager", &properties.manager),
            ("company", &properties.company),
            ("presentation format", &properties.presentation_format),
            ("application", &properties.application),
            ("application version", &properties.application_version),
        ] {
            if let Some(value) = value {
                oxoffice_core::xml::reject_non_xml_characters(
                    &format!("application property {name}"),
                    value,
                )?;
            }
        }
        if let Some(version) = properties.application_version.as_deref()
            && !is_app_version(version)
        {
            return Err(Error::Other(format!(
                "application version {version:?} must have the form XX.YYYY \
                 (two digits, a dot, four digits), which Word requires to open the file"
            )));
        }
        self.edit_properties(|document| {
            document.reserve_application_properties_bundle()?;
            document.application_properties = Some(properties.clone());
            Ok(())
        })
    }

    /// Remove the complete application-properties part and package relationship.
    pub fn remove_application_properties(&mut self) -> Result<Option<AppProperties>> {
        let Some(properties) = self.application_properties.clone() else {
            return Ok(None);
        };
        let mut candidate = self.clone_for_staging();
        if let Some(part_name) = candidate.application_properties_part_name.take() {
            candidate.remove_package_properties_bundle(&part_name, rel_types::EXTENDED_PROPERTIES);
        }
        candidate.application_properties = None;
        self.commit_staged_mutation(candidate);
        Ok(Some(properties))
    }

    /// Return all custom properties in package order.
    pub fn custom_properties(&self) -> &[CustomProperty] {
        self.custom_properties
            .as_ref()
            .map(|properties| properties.properties.as_slice())
            .unwrap_or_default()
    }

    /// Return a custom property by its case-sensitive name.
    pub fn custom_property(&self, name: &str) -> Option<&CustomProperty> {
        self.custom_properties()
            .iter()
            .find(|property| property.name.as_deref() == Some(name))
    }

    /// Add or replace one named custom property.
    pub fn set_custom_property(&mut self, property: CustomProperty) -> Result<()> {
        let Some(name) = property.name.as_deref().filter(|name| !name.is_empty()) else {
            return Err(Error::Other(
                "an authored custom property requires a non-empty name".to_owned(),
            ));
        };
        oxoffice_core::xml::reject_non_xml_characters("a custom property name", name)?;
        oxoffice_core::xml::reject_non_xml_characters(
            &format!("the format id of custom property {name:?}"),
            &property.fmtid,
        )?;
        if let crate::CustomPropertyValue::Raw(raw) = &property.value {
            oxoffice_core::xml::reject_non_xml_characters(
                &format!("custom property {name:?}"),
                &String::from_utf8_lossy(raw),
            )?;
        }
        if let crate::CustomPropertyValue::Lpstr(value)
        | crate::CustomPropertyValue::Lpwstr(value)
        | crate::CustomPropertyValue::FileTime(value) = &property.value
        {
            oxoffice_core::xml::reject_non_xml_characters(
                &format!("custom property {name:?}"),
                value,
            )?;
        }
        if property.pid < 2 {
            return Err(Error::Other(
                "an authored custom property pid must be at least 2".to_owned(),
            ));
        }
        if self
            .custom_properties()
            .iter()
            .any(|existing| existing.name.as_deref() != Some(name) && existing.pid == property.pid)
        {
            return Err(Error::Other(format!(
                "custom property pid {} is already in use",
                property.pid
            )));
        }

        self.edit_properties(|document| {
            let created = document.custom_properties_part_name.is_none();
            document.reserve_custom_properties_bundle()?;
            let properties = document
                .custom_properties
                .get_or_insert_with(CustomProperties::default);
            if let Some(index) = properties
                .properties
                .iter()
                .position(|existing| existing.name.as_deref() == Some(name))
            {
                properties
                    .properties
                    .retain(|existing| existing.name.as_deref() != Some(name));
                properties.properties.insert(index, property.clone());
            } else {
                properties.properties.push(property.clone());
            }
            document.custom_properties_owned |= created;
            Ok(())
        })
    }

    /// Remove one named custom property.
    pub fn remove_custom_property(&mut self, name: &str) -> Result<Option<CustomProperty>> {
        let Some(removed) = self
            .custom_properties()
            .iter()
            .find(|property| property.name.as_deref() == Some(name))
            .cloned()
        else {
            return Ok(None);
        };
        self.edit_properties(|document| {
            let properties = document
                .custom_properties
                .as_mut()
                .expect("custom property lookup proved the model exists");
            properties
                .properties
                .retain(|property| property.name.as_deref() != Some(name));
            if properties.properties.is_empty() && document.custom_properties_owned {
                if let Some(part_name) = document.custom_properties_part_name.take() {
                    document
                        .remove_package_properties_bundle(&part_name, rel_types::CUSTOM_PROPERTIES);
                }
                document.custom_properties = None;
                document.custom_properties_owned = false;
            }
            Ok(())
        })?;
        Ok(Some(removed))
    }

    /// Remove the complete custom-properties part and package relationship.
    pub fn remove_custom_properties(&mut self) -> Result<Option<Vec<CustomProperty>>> {
        let Some(properties) = self.custom_properties.clone() else {
            return Ok(None);
        };
        let mut candidate = self.clone_for_staging();
        if let Some(part_name) = candidate.custom_properties_part_name.take() {
            candidate.remove_package_properties_bundle(&part_name, rel_types::CUSTOM_PROPERTIES);
        }
        candidate.custom_properties = None;
        candidate.custom_properties_owned = false;
        self.commit_staged_mutation(candidate);
        Ok(Some(properties.properties))
    }

    fn remove_package_properties_bundle(&mut self, part_name: &str, rel_type: &str) {
        let mut removed_ids = Vec::new();
        self.package.package_rels.items.retain(|relationship| {
            let remove = relationship.rel_type == rel_type
                && relationship_is_internal(relationship)
                && OpcPackage::resolve_rel_target("/", &relationship.target) == part_name;
            if remove {
                removed_ids.push(relationship.id.clone());
            }
            !remove
        });
        self.identifiers
            .retire_authored_story_relationships("/", removed_ids);
        self.package.remove_part(part_name);
        self.package.remove_part_rels(part_name);
        self.package.content_types.remove_override(part_name);
        self.identifiers.retire_authored_part(part_name);
    }

    /// Get the document title.
    pub fn title(&self) -> Option<&str> {
        self.core_properties.as_ref()?.title.as_deref()
    }

    /// Set the document title.
    ///
    /// A character XML 1.0 cannot carry is taken as given, and every later save
    /// then fails with the error of the package writer, which names the part
    /// (`/docProps/core.xml`) and the character; [`Self::set_core_properties`]
    /// refuses such a value instead.
    pub fn set_title(&mut self, title: &str) {
        let mut candidate = self.clone_for_staging();
        candidate
            .reserve_core_properties_bundle()
            .expect("an in-memory document can allocate a core-properties part");
        candidate.invalidate_layout();
        candidate.ensure_core_properties().title = Some(title.to_string());
        self.commit_staged_mutation(candidate);
    }

    /// Get the document author/creator.
    pub fn author(&self) -> Option<&str> {
        self.core_properties.as_ref()?.creator.as_deref()
    }

    /// Set the document author/creator.
    ///
    /// A character XML 1.0 cannot carry is taken as given, and every later save
    /// then fails with the error of the package writer, which names the part
    /// (`/docProps/core.xml`) and the character; [`Self::set_core_properties`]
    /// refuses such a value instead.
    pub fn set_author(&mut self, author: &str) {
        let mut candidate = self.clone_for_staging();
        candidate
            .reserve_core_properties_bundle()
            .expect("an in-memory document can allocate a core-properties part");
        candidate.invalidate_layout();
        candidate.ensure_core_properties().creator = Some(author.to_string());
        self.commit_staged_mutation(candidate);
    }

    /// Get the document subject.
    pub fn subject(&self) -> Option<&str> {
        self.core_properties.as_ref()?.subject.as_deref()
    }

    /// Set the document subject.
    ///
    /// A character XML 1.0 cannot carry is taken as given, and every later save
    /// then fails with the error of the package writer, which names the part
    /// (`/docProps/core.xml`) and the character; [`Self::set_core_properties`]
    /// refuses such a value instead.
    pub fn set_subject(&mut self, subject: &str) {
        let mut candidate = self.clone_for_staging();
        candidate
            .reserve_core_properties_bundle()
            .expect("an in-memory document can allocate a core-properties part");
        candidate.invalidate_layout();
        candidate.ensure_core_properties().subject = Some(subject.to_string());
        self.commit_staged_mutation(candidate);
    }

    /// Get the document keywords.
    pub fn keywords(&self) -> Option<&str> {
        self.core_properties.as_ref()?.keywords.as_deref()
    }

    /// Set the document keywords.
    ///
    /// A character XML 1.0 cannot carry is taken as given, and every later save
    /// then fails with the error of the package writer, which names the part
    /// (`/docProps/core.xml`) and the character; [`Self::set_core_properties`]
    /// refuses such a value instead.
    pub fn set_keywords(&mut self, keywords: &str) {
        let mut candidate = self.clone_for_staging();
        candidate
            .reserve_core_properties_bundle()
            .expect("an in-memory document can allocate a core-properties part");
        candidate.invalidate_layout();
        candidate.ensure_core_properties().keywords = Some(keywords.to_string());
        self.commit_staged_mutation(candidate);
    }

    fn ensure_core_properties(&mut self) -> &mut CoreProperties {
        self.core_properties.get_or_insert_default()
    }
}
