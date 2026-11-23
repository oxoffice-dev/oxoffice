//! Writing one plot, matching its original series and axes to the current ones.

use super::*;

pub(super) fn write_plot<W: Write>(
    writer: &mut Writer<W>,
    plot: &Plot,
    markup: &PlotMarkup,
) -> Result<()> {
    plot.validate()?;
    if let Some(parsed_bar) = markup.parsed_bar
        && parsed_bar != matches!(plot, Plot::Bar { .. })
    {
        return Err(ChartError::InvalidValue {
            element: "c:plotArea".to_owned(),
            value: "a parsed plot family cannot be replaced while preserved payload remains"
                .to_owned(),
        });
    }
    let current_remaining = match plot {
        Plot::Pie { .. } => Some("pieChart"),
        Plot::Doughnut { .. } => Some("doughnutChart"),
        Plot::Area { .. } => Some("areaChart"),
        Plot::Scatter { .. } => Some("scatterChart"),
        Plot::Radar { .. } => Some("radarChart"),
        Plot::Bar { .. } | Plot::Line { .. } => None,
    };
    if let Some(parsed) = markup.parsed_remaining
        && current_remaining != Some(parsed)
    {
        return Err(ChartError::InvalidValue {
            element: "c:plotArea".to_owned(),
            value: "a parsed plot family cannot be replaced while preserved payload remains"
                .to_owned(),
        });
    }
    match plot {
        Plot::Bar {
            direction,
            grouping,
            gap_width,
            overlap,
            series,
            data_labels,
            axis_ids,
        } => {
            let mut start = BytesStart::new("c:barChart");
            push_attributes(&mut start, &markup.raw_attributes);
            writer
                .write_event(Event::Start(start))
                .map_err(OxmlError::from)?;
            emit_raw(writer, markup.raw_children.at(0))?;
            write_scalar(
                writer,
                "c:barDir",
                direction.as_str(),
                markup.direction.as_ref(),
            )?;
            emit_raw(writer, markup.raw_children.at(1))?;
            write_scalar(
                writer,
                "c:grouping",
                grouping.as_str(),
                markup.grouping.as_ref(),
            )?;
            let original_to_current =
                series_original_to_current(&markup.original_series_keys, series);
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                2,
                &original_to_current,
                series.len(),
                0,
            )?;
            for (index, item) in series.iter().enumerate() {
                writer
                    .get_mut()
                    .write_all(&item.to_xml()?)
                    .map_err(OxmlError::from)?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    2,
                    &original_to_current,
                    series.len(),
                    index + 1,
                )?;
            }
            if let Some(labels) = data_labels {
                writer
                    .get_mut()
                    .write_all(&labels.to_xml()?)
                    .map_err(OxmlError::from)?;
            }
            let trailing = markup.original_series_keys.len();
            emit_raw(writer, markup.raw_children.at(trailing + 3))?;
            if *gap_width != 150 || markup.gap_width.is_some() {
                write_scalar(
                    writer,
                    "c:gapWidth",
                    &gap_width.to_string(),
                    markup.gap_width.as_ref(),
                )?;
            }
            emit_raw(writer, markup.raw_children.at(trailing + 4))?;
            if *overlap != 0 || markup.overlap.is_some() {
                write_scalar(
                    writer,
                    "c:overlap",
                    &overlap.to_string(),
                    markup.overlap.as_ref(),
                )?;
            }
            let axis_original_to_current =
                axis_original_to_current(&markup.original_axis_ids, axis_ids);
            let axis_current_to_original =
                invert_original_to_current(&axis_original_to_current, axis_ids.len());
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                trailing + 5,
                &axis_original_to_current,
                axis_ids.len(),
                0,
            )?;
            for (index, id) in axis_ids.iter().enumerate() {
                let default_markup = default_axis_id_markup(*id);
                let original_markup = axis_current_to_original[index]
                    .and_then(|original| markup.axis_ids.get(original));
                write_axis_id(
                    writer,
                    "c:axId",
                    *id,
                    original_markup.unwrap_or(&default_markup),
                )?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    trailing + 5,
                    &axis_original_to_current,
                    axis_ids.len(),
                    index + 1,
                )?;
            }
            writer
                .write_event(Event::End(BytesEnd::new("c:barChart")))
                .map_err(OxmlError::from)?;
        }
        Plot::Line {
            grouping,
            marker,
            smooth,
            series,
            data_labels,
            axis_ids,
        } => {
            let mut start = BytesStart::new("c:lineChart");
            push_attributes(&mut start, &markup.raw_attributes);
            writer
                .write_event(Event::Start(start))
                .map_err(OxmlError::from)?;
            emit_raw(writer, markup.raw_children.at(0))?;
            write_scalar(
                writer,
                "c:grouping",
                grouping.as_str(),
                markup.grouping.as_ref(),
            )?;
            let original_to_current =
                series_original_to_current(&markup.original_series_keys, series);
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                1,
                &original_to_current,
                series.len(),
                0,
            )?;
            for (index, item) in series.iter().enumerate() {
                writer
                    .get_mut()
                    .write_all(&item.to_xml()?)
                    .map_err(OxmlError::from)?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    1,
                    &original_to_current,
                    series.len(),
                    index + 1,
                )?;
            }
            if let Some(labels) = data_labels {
                writer
                    .get_mut()
                    .write_all(&labels.to_xml()?)
                    .map_err(OxmlError::from)?;
            }
            let trailing = markup.original_series_keys.len();
            emit_raw(writer, markup.raw_children.at(trailing + 2))?;
            if *marker || markup.marker.is_some() {
                write_scalar(
                    writer,
                    "c:marker",
                    bool_lexical(*marker),
                    markup.marker.as_ref(),
                )?;
            }
            emit_raw(writer, markup.raw_children.at(trailing + 3))?;
            if *smooth || markup.smooth.is_some() {
                write_scalar(
                    writer,
                    "c:smooth",
                    bool_lexical(*smooth),
                    markup.smooth.as_ref(),
                )?;
            }
            let axis_original_to_current =
                axis_original_to_current(&markup.original_axis_ids, axis_ids);
            let axis_current_to_original =
                invert_original_to_current(&axis_original_to_current, axis_ids.len());
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                trailing + 4,
                &axis_original_to_current,
                axis_ids.len(),
                0,
            )?;
            for (index, id) in axis_ids.iter().enumerate() {
                let default_markup = default_axis_id_markup(*id);
                let original_markup = axis_current_to_original[index]
                    .and_then(|original| markup.axis_ids.get(original));
                write_axis_id(
                    writer,
                    "c:axId",
                    *id,
                    original_markup.unwrap_or(&default_markup),
                )?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    trailing + 4,
                    &axis_original_to_current,
                    axis_ids.len(),
                    index + 1,
                )?;
            }
            writer
                .write_event(Event::End(BytesEnd::new("c:lineChart")))
                .map_err(OxmlError::from)?;
        }
        remaining => {
            let (
                root,
                property,
                series,
                data_labels,
                axis_ids,
                first_slice_angle,
                hole_size,
                scatter,
            ) = match remaining {
                Plot::Pie {
                    first_slice_angle,
                    series,
                    data_labels,
                } => (
                    "pieChart",
                    None,
                    series,
                    data_labels,
                    None,
                    Some(*first_slice_angle),
                    None,
                    false,
                ),
                Plot::Doughnut {
                    first_slice_angle,
                    hole_size,
                    series,
                    data_labels,
                } => (
                    "doughnutChart",
                    None,
                    series,
                    data_labels,
                    None,
                    Some(*first_slice_angle),
                    Some(*hole_size),
                    false,
                ),
                Plot::Area {
                    grouping,
                    series,
                    data_labels,
                    axis_ids,
                } => (
                    "areaChart",
                    Some(("c:grouping", grouping.as_str(), markup.grouping.as_ref())),
                    series,
                    data_labels,
                    Some(axis_ids),
                    None,
                    None,
                    false,
                ),
                Plot::Scatter {
                    style,
                    series,
                    data_labels,
                    axis_ids,
                } => (
                    "scatterChart",
                    Some(("c:scatterStyle", style.as_str(), markup.style.as_ref())),
                    series,
                    data_labels,
                    Some(axis_ids),
                    None,
                    None,
                    true,
                ),
                Plot::Radar {
                    style,
                    series,
                    data_labels,
                    axis_ids,
                } => (
                    "radarChart",
                    Some(("c:radarStyle", style.as_str(), markup.style.as_ref())),
                    series,
                    data_labels,
                    Some(axis_ids),
                    None,
                    None,
                    false,
                ),
                Plot::Bar { .. } | Plot::Line { .. } => unreachable!("handled above"),
            };
            let tag = format!("c:{root}");
            let mut start = BytesStart::new(&tag);
            push_attributes(&mut start, &markup.raw_attributes);
            writer
                .write_event(Event::Start(start))
                .map_err(OxmlError::from)?;
            let base = usize::from(property.is_some());
            if let Some((name, value, scalar_markup)) = property {
                emit_raw(writer, markup.raw_children.at(0))?;
                write_scalar(writer, name, value, scalar_markup)?;
            }
            let original_to_current =
                series_original_to_current(&markup.original_series_keys, series);
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                base,
                &original_to_current,
                series.len(),
                0,
            )?;
            for (index, item) in series.iter().enumerate() {
                writer
                    .get_mut()
                    .write_all(&item.to_xml_for_plot(scatter)?)
                    .map_err(OxmlError::from)?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    base,
                    &original_to_current,
                    series.len(),
                    index + 1,
                )?;
            }
            if let Some(labels) = data_labels {
                writer
                    .get_mut()
                    .write_all(&labels.to_xml()?)
                    .map_err(OxmlError::from)?;
            }
            let trailing = base + markup.original_series_keys.len();
            if let Some(axis_ids) = axis_ids {
                let axis_original_to_current =
                    axis_original_to_current(&markup.original_axis_ids, axis_ids);
                let axis_current_to_original =
                    invert_original_to_current(&axis_original_to_current, axis_ids.len());
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    trailing + 1,
                    &axis_original_to_current,
                    axis_ids.len(),
                    0,
                )?;
                for (index, id) in axis_ids.iter().enumerate() {
                    let default_markup = default_axis_id_markup(*id);
                    let original_markup = axis_current_to_original[index]
                        .and_then(|original| markup.axis_ids.get(original));
                    write_axis_id(
                        writer,
                        "c:axId",
                        *id,
                        original_markup.unwrap_or(&default_markup),
                    )?;
                    emit_repeated_raw(
                        writer,
                        &markup.raw_children,
                        trailing + 1,
                        &axis_original_to_current,
                        axis_ids.len(),
                        index + 1,
                    )?;
                }
            } else {
                emit_raw(writer, markup.raw_children.at(trailing + 1))?;
                if let Some(angle) = first_slice_angle {
                    if angle != 0 || markup.first_slice_angle.is_some() {
                        write_scalar(
                            writer,
                            "c:firstSliceAng",
                            &angle.to_string(),
                            markup.first_slice_angle.as_ref(),
                        )?;
                    }
                    emit_raw(writer, markup.raw_children.at(trailing + 2))?;
                }
                if let Some(size) = hole_size {
                    if size != 50 || markup.hole_size.is_some() {
                        write_scalar(
                            writer,
                            "c:holeSize",
                            &size.to_string(),
                            markup.hole_size.as_ref(),
                        )?;
                    }
                    emit_raw(writer, markup.raw_children.at(trailing + 3))?;
                }
            }
            writer
                .write_event(Event::End(BytesEnd::new(&tag)))
                .map_err(OxmlError::from)?;
        }
    }
    Ok(())
}

fn series_original_to_current(original: &[(u32, u32)], current: &[Series]) -> Vec<Option<usize>> {
    let mut used = vec![false; current.len()];
    let mut matches = vec![None; original.len()];
    for original_index in 0..original.len().min(current.len()) {
        if original[original_index]
            == (current[original_index].index, current[original_index].order)
        {
            matches[original_index] = Some(original_index);
            used[original_index] = true;
        }
    }
    for (original_index, key) in original.iter().enumerate() {
        if matches[original_index].is_some() {
            continue;
        }
        if let Some(current_index) = current
            .iter()
            .enumerate()
            .position(|(index, item)| !used[index] && (item.index, item.order) == *key)
        {
            matches[original_index] = Some(current_index);
            used[current_index] = true;
        }
    }
    match_unidentified_by_position(&mut matches, &mut used);
    matches
}

fn axis_original_to_current(original: &[AxisId], current: &[AxisId]) -> Vec<Option<usize>> {
    let mut used = vec![false; current.len()];
    let mut matches = vec![None; original.len()];
    for (original_index, id) in original.iter().enumerate() {
        if let Some(current_index) = current
            .iter()
            .enumerate()
            .position(|(index, current_id)| !used[index] && current_id == id)
        {
            matches[original_index] = Some(current_index);
            used[current_index] = true;
        }
    }
    match_unidentified_by_position(&mut matches, &mut used);
    matches
}

fn match_unidentified_by_position(matches: &mut [Option<usize>], used: &mut [bool]) {
    for original_index in 0..matches.len() {
        if matches[original_index].is_none() && original_index < used.len() && !used[original_index]
        {
            matches[original_index] = Some(original_index);
            used[original_index] = true;
        }
    }
    for matched in matches.iter_mut().filter(|matched| matched.is_none()) {
        if let Some(current_index) = used.iter().position(|used| !used) {
            *matched = Some(current_index);
            used[current_index] = true;
        }
    }
}

fn invert_original_to_current(
    original_to_current: &[Option<usize>],
    current_len: usize,
) -> Vec<Option<usize>> {
    let mut current_to_original = vec![None; current_len];
    for (original, current) in original_to_current.iter().enumerate() {
        if let Some(current) = current {
            current_to_original[*current] = Some(original);
        }
    }
    current_to_original
}

fn emit_repeated_raw<W: Write>(
    writer: &mut Writer<W>,
    raw_children: &OrderedRawChildren,
    offset: usize,
    original_to_current: &[Option<usize>],
    current_len: usize,
    current_boundary: usize,
) -> Result<()> {
    if current_boundary == 0 {
        emit_raw(writer, raw_children.at(offset))?;
    }
    for original_boundary in 1..=original_to_current.len() {
        let effective = original_to_current
            .iter()
            .skip(original_boundary)
            .flatten()
            .copied()
            .next()
            .unwrap_or(current_len);
        if effective == current_boundary {
            emit_raw(writer, raw_children.at(offset + original_boundary))?;
        }
    }
    Ok(())
}
