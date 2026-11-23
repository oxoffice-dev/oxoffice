//! The legend: positions, effective settings, parsing and writing.

use super::*;

/// The five ChartML legend positions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LegendPosition {
    #[default]
    Right,
    Left,
    Top,
    Bottom,
    TopRight,
}

impl LegendPosition {
    /// The ChartML lexical value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "r",
            Self::Left => "l",
            Self::Top => "t",
            Self::Bottom => "b",
            Self::TopRight => "tr",
        }
    }
    /// Parse one of the five supported lexical values.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "r" => Ok(Self::Right),
            "l" => Ok(Self::Left),
            "t" => Ok(Self::Top),
            "b" => Ok(Self::Bottom),
            "tr" => Ok(Self::TopRight),
            _ => Err(invalid_authoring_value(format!(
                "unknown legend position {value}"
            ))),
        }
    }
}

/// Effective legend settings. An absent legend has right/false defaults.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LegendSettings {
    pub visible: bool,
    pub position: LegendPosition,
    pub overlay: bool,
}

impl Default for CT_Legend {
    fn default() -> Self {
        Self {
            root_name: "c:legend".into(),
            settings: LegendSettings {
                visible: true,
                ..LegendSettings::default()
            },
            raw_attributes: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }
}

impl CT_Legend {
    pub(super) fn from_xml_with_namespaces(
        xml: &[u8],
        namespaces: &NamespaceBindings,
    ) -> Result<Self> {
        let mut wrapper = BytesStart::new("scope");
        for (prefix, uri) in namespaces {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{}", String::from_utf8_lossy(prefix))
            };
            wrapper.push_attribute((name.as_str(), uri.as_str()));
        }
        let mut writer = Writer::new(Vec::new());
        writer
            .write_event(Event::Start(wrapper))
            .map_err(OxmlError::from)?;
        writer.get_mut().extend_from_slice(xml);
        writer
            .write_event(Event::End(BytesEnd::new("scope")))
            .map_err(OxmlError::from)?;
        let wrapped = writer.into_inner();
        let nodes = chart_source_nodes(&wrapped)?;
        let legend = chart_child_node(&nodes, 0, b"legend")?;
        let settings = legend_settings_at(&wrapped, &nodes, legend)?;
        let (raw_attributes, raw_children) = parse_raw_shell(xml, b"legend", "c:legend")?;
        let mut reader = Reader::from_reader(xml);
        let root_name = loop {
            match reader.read_event().map_err(OxmlError::from)? {
                Event::Start(e) | Event::Empty(e) => break element_name(&e),
                Event::Eof => return Err(missing_end("legend")),
                _ => {}
            }
        };
        // Preserve a producer root alias only when a foreign c binding must
        // remain in scope for opaque descendants. Otherwise retain fixed c output.
        let root_name = if raw_attributes
            .iter()
            .any(|(name, uri)| name == "xmlns:c" && uri != C_NS)
        {
            root_name
        } else {
            "c:legend".to_owned()
        };
        Ok(Self {
            root_name,
            settings,
            raw_attributes,
            raw_children,
        })
    }

    pub(super) fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        write_raw_shell(
            writer,
            &self.root_name,
            &self.raw_attributes,
            &self.raw_children,
        )
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}
