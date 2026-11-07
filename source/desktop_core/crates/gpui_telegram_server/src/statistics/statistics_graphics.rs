//! Telegram Desktop 1:1 Submodule: `statistics_graphics`
//! Subsystem: `statistics`, Feature ID: `TCG-N12`

use crate::statistics::statistics_data_deserialize::{StatisticalChart, StatisticalCurrency};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrencyIconDescriptor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub color_tint: Option<String>,
}

pub fn chart_currency_icon(
    chart_data: &StatisticalChart,
    color: Option<String>,
) -> Option<CurrencyIconDescriptor> {
    match chart_data.currency {
        Some(StatisticalCurrency::Ton) => Some(CurrencyIconDescriptor {
            name: "statistics/mini_currency_graph".to_string(),
            width: 14,
            height: 14,
            color_tint: color,
        }),
        Some(StatisticalCurrency::Credits) => Some(CurrencyIconDescriptor {
            name: "menu/gift_premium".to_string(),
            width: 14,
            height: 14,
            color_tint: color,
        }),
        None => None,
    }
}

/// Target symbol wrapper for compatibility
pub struct StatisticsGraphics;

impl StatisticsGraphics {
    pub fn currency_icon(
        chart_data: &StatisticalChart,
        color: Option<String>,
    ) -> Option<CurrencyIconDescriptor> {
        chart_currency_icon(chart_data, color)
    }
}
