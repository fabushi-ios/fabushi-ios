//! Telegram Desktop 1:1 Submodule: `statistics_sheets`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_data_deserialize::StatisticalChart;
use crate::statistics::statistics_xlsx::{Cell, Sheet};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StatisticalValue {
    pub value: f64,
    pub previous_value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StatisticalGraph {
    pub chart: Option<StatisticalChart>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ChannelStatistics {
    pub member_count: StatisticalValue,
    pub mean_view_count: StatisticalValue,
    pub mean_share_count: StatisticalValue,
    pub mean_reaction_count: StatisticalValue,
    pub mean_story_view_count: StatisticalValue,
    pub mean_story_share_count: StatisticalValue,
    pub mean_story_reaction_count: StatisticalValue,
    pub enabled_notifications_percentage: f64,
    pub member_count_graph: StatisticalGraph,
    pub join_graph: StatisticalGraph,
    pub mute_graph: StatisticalGraph,
    pub view_count_by_hour_graph: StatisticalGraph,
    pub view_count_by_source_graph: StatisticalGraph,
    pub join_by_source_graph: StatisticalGraph,
    pub language_graph: StatisticalGraph,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SupergroupStatistics {
    pub member_count: StatisticalValue,
    pub message_count: StatisticalValue,
    pub viewer_count: StatisticalValue,
    pub sender_count: StatisticalValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PostStatistics {
    pub views: f64,
    pub reactions: f64,
    pub public_forwards: f64,
    pub private_forwards: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AnyStatistics {
    pub channel: Option<ChannelStatistics>,
    pub supergroup: Option<SupergroupStatistics>,
    pub post: Option<PostStatistics>,
}

const K_ONE_DAY_MS: f64 = 86_400_000.0;

pub fn sheet_from_chart(title: &str, chart: &StatisticalChart) -> Sheet {
    let without_timestamps = chart.x.is_empty() || chart.x[0] < K_ONE_DAY_MS;
    let whole_day = chart.time_step >= K_ONE_DAY_MS;

    let mut header = vec![Cell::header(if without_timestamps { "Hour" } else { "Date" })];
    for line in &chart.lines {
        header.push(Cell::header(&line.name));
    }

    let mut rows = vec![header];
    for (i, &x_val) in chart.x.iter().enumerate() {
        let mut row = Vec::with_capacity(chart.lines.len() + 1);
        if without_timestamps {
            row.push(Cell::number(x_val));
        } else if whole_day {
            row.push(Cell::date(x_val));
        } else {
            row.push(Cell::date_time(x_val));
        }

        for line in &chart.lines {
            if i < line.y.len() {
                row.push(Cell::number(line.y[i] as f64));
            } else {
                row.push(Cell::empty());
            }
        }
        rows.push(row);
    }

    Sheet {
        name: title.to_string(),
        rows,
    }
}

pub fn sheets(stats: &AnyStatistics, _poll_votes: &StatisticalGraph) -> Vec<Sheet> {
    let mut result = Vec::new();
    let mut overview_rows = vec![vec![
        Cell::empty(),
        Cell::header("Value"),
        Cell::header("Previous"),
    ]];

    if let Some(channel) = &stats.channel {
        let add_val = |rows: &mut Vec<Vec<Cell>>, title: &str, val: &StatisticalValue| {
            rows.push(vec![
                Cell::text(title),
                Cell::number(val.value),
                Cell::number(val.previous_value),
            ]);
        };
        add_val(&mut overview_rows, "Members", &channel.member_count);
        add_val(&mut overview_rows, "Mean Views", &channel.mean_view_count);
        add_val(&mut overview_rows, "Mean Shares", &channel.mean_share_count);
        add_val(&mut overview_rows, "Mean Reactions", &channel.mean_reaction_count);
        add_val(&mut overview_rows, "Mean Story Views", &channel.mean_story_view_count);
        add_val(&mut overview_rows, "Mean Story Shares", &channel.mean_story_share_count);
        add_val(&mut overview_rows, "Mean Story Reactions", &channel.mean_story_reaction_count);
        overview_rows.push(vec![
            Cell::text("Notifications Enabled (%)"),
            Cell::number(channel.enabled_notifications_percentage),
            Cell::empty(),
        ]);

        result.push(Sheet {
            name: "Overview".to_string(),
            rows: overview_rows,
        });

        if let Some(chart) = &channel.member_count_graph.chart {
            result.push(sheet_from_chart("Growth", chart));
        }
        if let Some(chart) = &channel.join_graph.chart {
            result.push(sheet_from_chart("Followers", chart));
        }
        if let Some(chart) = &channel.view_count_by_hour_graph.chart {
            result.push(sheet_from_chart("Views by Hour", chart));
        }
    } else if let Some(supergroup) = &stats.supergroup {
        overview_rows.push(vec![
            Cell::text("Members"),
            Cell::number(supergroup.member_count.value),
            Cell::number(supergroup.member_count.previous_value),
        ]);
        overview_rows.push(vec![
            Cell::text("Messages"),
            Cell::number(supergroup.message_count.value),
            Cell::number(supergroup.message_count.previous_value),
        ]);
        result.push(Sheet {
            name: "Overview".to_string(),
            rows: overview_rows,
        });
    }

    result
}

/// Target symbol wrapper for compatibility
pub struct StatisticsSheets;

impl StatisticsSheets {
    pub fn generate_sheets(stats: &AnyStatistics, poll_votes: &StatisticalGraph) -> Vec<Sheet> {
        sheets(stats, poll_votes)
    }
}
