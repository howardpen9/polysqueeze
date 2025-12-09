use polysqueeze::Result;
use polysqueeze::client::ClobClient;
use polysqueeze::errors::PolyError;
use polysqueeze::types::{GammaListParams, Market};
use polysqueeze::wss::{WssMarketClient, WssMarketEvent};

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::env;
use std::io;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use chrono::{DateTime, Utc};

/// Format a number with comma separators for thousands
fn format_with_commas(num: Decimal) -> String {
    let num_str = format!("{:.0}", num);
    let mut result = String::new();
    let chars: Vec<char> = num_str.chars().collect();
    let len = chars.len();
    
    for (i, ch) in chars.iter().enumerate() {
        result.push(*ch);
        // Add comma every 3 digits from the right (but not at the end)
        if (len - i - 1) % 3 == 0 && i < len - 1 {
            result.push(',');
        }
    }
    result
}

/// Format price as cents (e.g., $0.0010 -> "0.1¢")
fn format_price_as_cents(price: Decimal) -> String {
    // Convert to cents: multiply by 100
    let cents = price * Decimal::from(100);
    // Format with 1 decimal place
    format!("{:.1}¢", cents)
}

/// Format dollar amount with commas and 2 decimal places
fn format_dollar_amount(amount: Decimal) -> String {
    let rounded = amount.round_dp(2);
    let integer_part = rounded.trunc();
    let decimal_part = (rounded - integer_part) * Decimal::from(100);
    let decimal_int = decimal_part.to_u64().unwrap_or(0);
    
    if decimal_int == 0 {
        format!("${}", format_with_commas(integer_part))
    } else {
        format!("${}.{:02}", format_with_commas(integer_part), decimal_int)
    }
}

/// Format a decimal number with comma separators for thousands (handles decimal places)
fn format_size_with_commas(size: Decimal) -> String {
    // Format with 2 decimal places
    let num_str = format!("{:.2}", size);
    
    // Split into integer and decimal parts
    let parts: Vec<&str> = num_str.split('.').collect();
    let integer_part = parts[0];
    let decimal_part = if parts.len() > 1 { parts[1] } else { "" };
    
    // Add commas to integer part
    let mut result = String::new();
    let chars: Vec<char> = integer_part.chars().collect();
    let len = chars.len();
    
    for (i, ch) in chars.iter().enumerate() {
        result.push(*ch);
        // Add comma every 3 digits from the right (but not at the end)
        if (len - i - 1) % 3 == 0 && i < len - 1 {
            result.push(',');
        }
    }
    
    // Add decimal part if exists
    if !decimal_part.is_empty() {
        result.push('.');
        result.push_str(decimal_part);
    }
    
    result
}

#[tokio::main]
async fn main() -> Result<()> {
    let base_url =
        env::var("POLY_API_URL").unwrap_or_else(|_| "https://clob.polymarket.com".to_string());
    let clob = ClobClient::new(&base_url);

    // 方式1: 直接指定 asset_ids (最高优先级)
    if let Ok(asset_ids_str) = env::var("POLY_ASSET_IDS") {
        let asset_ids: Vec<String> = asset_ids_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        
        if !asset_ids.is_empty() {
            println!("🎯 Using asset IDs from POLY_ASSET_IDS environment variable");
            println!("Asset IDs: {:?}\n", asset_ids);
            
            let mut client = WssMarketClient::new();
            client.subscribe(asset_ids.clone()).await?;
            
            println!("✅ Subscribed to market channel for assets={:?}\n", asset_ids);
            println!("🔄 Receiving real-time updates...\n");
            
            // 接收事件（可以设置为持续运行或限制次数）
            // 0 或未设置 = 无限循环，否则限制事件数量
            let event_limit: Option<usize> = env::var("POLY_WSS_EVENT_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&n| n > 0); // 如果是 0，转换为 None（无限循环）
            
            handle_events(&mut client, event_limit).await?;
            
            return Ok(());
        }
    }

    // 方式2: 通过 condition_id 指定特定市场
    let market = if let Ok(condition_id) = env::var("POLY_CONDITION_ID") {
        println!("🎯 Loading market by condition_id: {}", condition_id);
        clob.get_market(&condition_id).await?
    } else {
        // 方式3: 使用 TUI 让用户选择市场
        let min_liquidity = env::var("POLY_WSS_MIN_LIQUIDITY")
            .ok()
            .and_then(|value| Decimal::from_str(&value).ok())
            .unwrap_or_else(|| Decimal::from(10_000));

        let params = GammaListParams {
            limit: Some(100), // 获取更多市场供选择
            liquidity_num_min: Some(min_liquidity),
            ..Default::default()
        };

        let response = clob.get_markets(None, Some(&params)).await?;
        // 直接使用从列表中选择的市场，不需要再次调用 API
        select_market_tui(&response.data).await?
    };

    // 在开头显示完整的市场信息
    println!("═══════════════════════════════════════════════════════════");
    println!("📊 Market Information");
    println!("═══════════════════════════════════════════════════════════");
    println!("Question: {}", market.question);
    if !market.description.is_empty() {
        println!("Description: {}", market.description);
    }
    println!("Condition ID: {}", market.condition_id);
    println!("Market Slug: {}", market.market_slug);
    if let Some(category) = &market.category {
        println!("Category: {}", category);
    }
    if let Some(end_date) = &market.end_date_iso {
        println!("End Date: {}", end_date);
    }
    if let Some(liq) = market.liquidity_num {
        println!("Liquidity: ${}", format_with_commas(liq));
    } else {
        println!("Liquidity: N/A");
    }
    println!("Active: {} | Closed: {}", market.active, market.closed);
    println!("═══════════════════════════════════════════════════════════\n");

    // 获取两个资产的 ID（Yes 和 No）
    let asset_ids = derive_asset_ids(&market).ok_or_else(|| {
        PolyError::validation("Failed to derive asset IDs for the selected market")
    })?;

    if asset_ids.len() < 2 {
        return Err(PolyError::validation("Market does not have both Yes and No tokens"));
    }

    let yes_asset_id = asset_ids[0].clone();
    let no_asset_id = asset_ids[1].clone();
    
    let yes_token = &market.tokens[0];
    let no_token = &market.tokens[1];

    let mut client = WssMarketClient::new();
    client.subscribe(asset_ids.clone()).await?;

    println!("✅ Subscribed to market channel for assets: Yes={} No={}\n", 
        &yes_asset_id[..20], &no_asset_id[..20]);
    println!("🔄 Starting real-time orderbook monitor for both assets...\n");
    
    // 使用实时 TUI 显示两个资产的订单簿
    run_realtime_tui(&market, &yes_asset_id, &no_asset_id, yes_token.outcome.as_str(), no_token.outcome.as_str(), client).await?;

    Ok(())
}

/// 单个资产的订单簿数据
#[derive(Clone)]
struct AssetBookData {
    bids: Vec<polysqueeze::types::OrderSummary>,
    asks: Vec<polysqueeze::types::OrderSummary>,
    recent_trades: Vec<(DateTime<Utc>, polysqueeze::wss::LastTradeMessage, Option<String>)>, // Added hash option
    best_bid: Option<Decimal>,
    best_ask: Option<Decimal>,
    // Store recent hashes from PriceChange messages to associate with trades
    recent_hashes: std::collections::HashMap<String, String>, // asset_id -> latest hash
}

impl AssetBookData {
    fn new() -> Self {
        Self {
            bids: Vec::new(),
            asks: Vec::new(),
            recent_trades: Vec::new(),
            best_bid: None,
            best_ask: None,
            recent_hashes: std::collections::HashMap::new(),
        }
    }

    fn update_book(&mut self, book: &polysqueeze::wss::MarketBook) {
        self.bids = book.bids.clone();
        self.asks = book.asks.clone();
        
        // 更新最佳买卖价
        self.best_bid = self.bids.first().map(|b| b.price);
        self.best_ask = self.asks.first().map(|a| a.price);
        
        // Store the hash from MarketBook - this is the transaction hash for the orderbook update
        if !book.hash.is_empty() {
            self.recent_hashes.insert(book.asset_id.clone(), book.hash.clone());
        }
    }

    fn add_trade(&mut self, trade: polysqueeze::wss::LastTradeMessage) {
        let now = Utc::now();
        // Get the most recent hash for this asset_id
        // This should be from MarketBook or PriceChange events that occurred before this trade
        let hash = self.recent_hashes.get(&trade.asset_id).cloned();
        self.recent_trades.insert(0, (now, trade, hash));
        // 只保留最近的 50 笔交易
        if self.recent_trades.len() > 50 {
            self.recent_trades.truncate(50);
        }
    }

    fn update_hash(&mut self, asset_id: &str, hash: String) {
        self.recent_hashes.insert(asset_id.to_string(), hash);
        // Keep only recent hashes (limit map size)
        if self.recent_hashes.len() > 10 {
            let oldest_key = self.recent_hashes.keys().next().cloned();
            if let Some(key) = oldest_key {
                self.recent_hashes.remove(&key);
            }
        }
    }
}

/// 实时订单簿和活动数据（包含两个资产）
struct RealtimeData {
    yes_data: AssetBookData,
    no_data: AssetBookData,
    yes_asset_id: String,
    no_asset_id: String,
    yes_selected: Option<usize>,  // Selected trade index for Yes asset
    no_selected: Option<usize>,   // Selected trade index for No asset
}

impl RealtimeData {
    fn new(yes_asset_id: String, no_asset_id: String) -> Self {
        Self {
            yes_data: AssetBookData::new(),
            no_data: AssetBookData::new(),
            yes_asset_id,
            no_asset_id,
            yes_selected: None,
            no_selected: None,
        }
    }

    fn get_selected_hash(&self, asset: &str) -> Option<String> {
        let (selected_idx, trades) = if asset == "yes" {
            (self.yes_selected, &self.yes_data.recent_trades)
        } else {
            (self.no_selected, &self.no_data.recent_trades)
        };

        if let Some(idx) = selected_idx {
            trades.get(idx).and_then(|(_, _, hash)| hash.clone())
        } else {
            None
        }
    }

    fn update_book(&mut self, book: &polysqueeze::wss::MarketBook) {
        if book.asset_id == self.yes_asset_id {
            self.yes_data.update_book(book);
        } else if book.asset_id == self.no_asset_id {
            self.no_data.update_book(book);
        }
    }

    fn add_trade(&mut self, trade: polysqueeze::wss::LastTradeMessage) {
        if trade.asset_id == self.yes_asset_id {
            self.yes_data.add_trade(trade);
        } else if trade.asset_id == self.no_asset_id {
            self.no_data.add_trade(trade);
        }
    }

    fn update_hash(&mut self, asset_id: &str, hash: String) {
        if asset_id == self.yes_asset_id {
            self.yes_data.update_hash(asset_id, hash);
        } else if asset_id == self.no_asset_id {
            self.no_data.update_hash(asset_id, hash);
        }
    }
}

/// 运行实时 TUI 显示订单簿和活动
async fn run_realtime_tui(
    market: &Market,
    yes_asset_id: &str,
    no_asset_id: &str,
    yes_label: &str,
    no_label: &str,
    mut client: WssMarketClient,
) -> Result<()> {
    let data = Arc::new(Mutex::new(RealtimeData::new(
        yes_asset_id.to_string(),
        no_asset_id.to_string(),
    )));
    let data_clone = Arc::clone(&data);
    let yes_asset_id_clone = yes_asset_id.to_string();
    let no_asset_id_clone = no_asset_id.to_string();

    // 启动事件处理任务
    let event_handle = tokio::spawn(async move {
        loop {
            match client.next_event().await {
                Ok(WssMarketEvent::Book(book)) => {
                    if book.asset_id == yes_asset_id_clone || book.asset_id == no_asset_id_clone {
                        if let Ok(mut data) = data_clone.lock() {
                            // Update book and store hash - MarketBook hash is the transaction hash
                            data.update_book(&book);
                        }
                    }
                }
                Ok(WssMarketEvent::LastTrade(trade)) => {
                    if trade.asset_id == yes_asset_id_clone || trade.asset_id == no_asset_id_clone {
                        if let Ok(mut data) = data_clone.lock() {
                            // Add trade - it will use the most recent hash from MarketBook or PriceChange
                            data.add_trade(trade);
                        }
                    }
                }
                Ok(WssMarketEvent::PriceChange(price_change)) => {
                    // Extract hash from price changes and store it
                    // Note: MarketBook hash will override this when it arrives (MarketBook is more accurate)
                    for change in price_change.price_changes {
                        if change.asset_id == yes_asset_id_clone || change.asset_id == no_asset_id_clone {
                            if let Ok(mut data) = data_clone.lock() {
                                data.update_hash(&change.asset_id, change.hash);
                            }
                        }
                    }
                }
                Ok(_) => {} // 忽略其他事件
                Err(err) => {
                    eprintln!("❌ WebSocket error: {}", err);
                    break;
                }
            }
        }
    });

    // Setup terminal for TUI
    enable_raw_mode().map_err(|e| PolyError::internal(format!("Failed to enable raw mode: {}", e), e))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| PolyError::internal(format!("Failed to setup terminal: {}", e), e))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend).map_err(|e| PolyError::internal(format!("Failed to create terminal: {}", e), e))?;

    // Track which side the user is navigating (Yes or No)
    let mut active_side = "yes"; // "yes" or "no"
    let mut yes_state = ListState::default();
    let mut no_state = ListState::default();
    yes_state.select(Some(0));
    no_state.select(Some(0));

    // 主 UI 循环
    let result = loop {
        {
            // Validate and fix selection indices before drawing
            if let Ok(data_guard) = data.try_lock() {
                // Fix Yes selection
                let yes_len = data_guard.yes_data.recent_trades.len();
                if yes_len == 0 {
                    yes_state.select(None);
                } else {
                    let yes_idx = yes_state.selected().unwrap_or(0);
                    if yes_idx >= yes_len {
                        yes_state.select(Some(yes_len.saturating_sub(1)));
                    } else if yes_state.selected().is_none() {
                        yes_state.select(Some(0));
                    }
                }
                
                // Fix No selection
                let no_len = data_guard.no_data.recent_trades.len();
                if no_len == 0 {
                    no_state.select(None);
                } else {
                    let no_idx = no_state.selected().unwrap_or(0);
                    if no_idx >= no_len {
                        no_state.select(Some(no_len.saturating_sub(1)));
                    } else if no_state.selected().is_none() {
                        no_state.select(Some(0));
                    }
                }
            }
            
            let yes_selected = yes_state.selected();
            let no_selected = no_state.selected();
            
            // Update selected indices in data and draw UI
            terminal.draw(|f| {
                // 注意：draw 回调是同步的，所以我们使用 try_lock
                // 如果锁被占用，就显示之前的状态
                if let Ok(mut data_guard) = data.try_lock() {
                    data_guard.yes_selected = yes_selected;
                    data_guard.no_selected = no_selected;
                    ui_realtime_sync(f, market, yes_label, no_label, &*data_guard, &mut yes_state, &mut no_state, active_side);
                }
            }).map_err(|e| PolyError::internal(format!("Failed to draw terminal: {}", e), e))?;
        }

        // 非阻塞检查按键和鼠标事件
        if event::poll(std::time::Duration::from_millis(100)).map_err(|e| PolyError::internal(format!("Failed to poll event: {}", e), e))? {
            match event::read().map_err(|e| PolyError::internal(format!("Terminal I/O error: {}", e), e))? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                break Ok(());
                            }
                            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                                // Switch between Yes and No sides
                                // Left goes to Yes, Right goes to No
                                let new_side = if key.code == KeyCode::Left {
                                    "yes"
                                } else if key.code == KeyCode::Right {
                                    "no"
                                } else {
                                    // Tab toggles
                                    if active_side == "yes" { "no" } else { "yes" }
                                };
                                
                                active_side = new_side;
                                
                                // Ensure selection index is valid for the new side
                                if let Ok(data_guard) = data.try_lock() {
                                    let (trades_len, state) = if new_side == "yes" {
                                        (data_guard.yes_data.recent_trades.len(), &mut yes_state)
                                    } else {
                                        (data_guard.no_data.recent_trades.len(), &mut no_state)
                                    };
                                    
                                    if trades_len == 0 {
                                        state.select(None);
                                    } else {
                                        let current = state.selected().unwrap_or(0);
                                        if current >= trades_len {
                                            state.select(Some(trades_len.saturating_sub(1)));
                                        } else if state.selected().is_none() {
                                            state.select(Some(0));
                                        }
                                    }
                                }
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                if let Ok(data_guard) = data.try_lock() {
                                    let (trades_len, state) = if active_side == "yes" {
                                        (data_guard.yes_data.recent_trades.len(), &mut yes_state)
                                    } else {
                                        (data_guard.no_data.recent_trades.len(), &mut no_state)
                                    };
                                    
                                    if trades_len > 0 {
                                        let i = state.selected().unwrap_or(0);
                                        let max = trades_len.saturating_sub(1);
                                        if i < max {
                                            state.select(Some(i + 1));
                                        }
                                    }
                                }
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                if let Ok(data_guard) = data.try_lock() {
                                    let (trades_len, state) = if active_side == "yes" {
                                        (data_guard.yes_data.recent_trades.len(), &mut yes_state)
                                    } else {
                                        (data_guard.no_data.recent_trades.len(), &mut no_state)
                                    };
                                    
                                    if trades_len > 0 {
                                        let i = state.selected().unwrap_or(0);
                                        if i > 0 {
                                            state.select(Some(i - 1));
                                        } else if state.selected().is_none() {
                                            // If no selection, select first item
                                            state.select(Some(0));
                                        }
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                // Enter key disabled - no action
                            }
                            _ => {}
                        }
                    }
                }
                Event::Mouse(_mouse_event) => {
                    // Mouse click disabled - no action
                }
                _ => {}
            }
        }

        // 检查事件处理任务是否完成
        if event_handle.is_finished() {
            break Ok(());
        }

        // 短暂延迟以控制刷新率
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    };

    // 取消事件处理任务
    event_handle.abort();

    // Restore terminal
    disable_raw_mode().map_err(|e| PolyError::internal(format!("Failed to disable raw mode: {}", e), e))?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    ).map_err(|e| PolyError::internal(format!("Failed to restore terminal: {}", e), e))?;
    terminal.show_cursor().map_err(|e| PolyError::internal(format!("Failed to show cursor: {}", e), e))?;

    result
}

/// 渲染实时 TUI (同步版本) - 显示两个资产的订单簿
fn ui_realtime_sync(
    f: &mut Frame,
    market: &Market,
    yes_label: &str,
    no_label: &str,
    data: &RealtimeData,
    yes_state: &mut ListState,
    no_state: &mut ListState,
    active_side: &str,
) {
    let size = f.area();
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(5),  // Header (增加高度以容纳 slug)
            Constraint::Min(10),    // Orderbook area (Yes and No)
            Constraint::Length(3),  // Footer
        ])
        .split(size);

    // Header - 添加 slug 信息（优化为可复制格式）
    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                format!("📊 {} | {} & {}", market.question, yes_label, no_label),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Slug: ", Style::default().fg(Color::Cyan)),
            // 只显示 slug，便于选择和复制
            Span::styled(
                &market.market_slug,
                Style::default().fg(Color::White).add_modifier(Modifier::UNDERLINED),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                "💡 Use mouse to select and copy",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            ),
        ]),
    ];
    
    let header = Paragraph::new(header_lines)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    // 分割为 Yes 和 No 两个部分
    let assets_layout = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // Yes Asset Orderbook
    render_asset_orderbook(f, &data.yes_data, yes_label, assets_layout[0], yes_state, active_side == "yes");

    // No Asset Orderbook
    render_asset_orderbook(f, &data.no_data, no_label, assets_layout[1], no_state, active_side == "no");

    // Footer - Add instructions
    let footer_text = format!("Q/ESC: Quit | Tab/←/→: Switch | ↑/↓: Navigate | Active: {}", 
            if active_side == "yes" { yes_label } else { no_label });
    
    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}

/// 渲染单个资产的订单簿
fn render_asset_orderbook(
    f: &mut Frame,
    data: &AssetBookData,
    label: &str,
    area: Rect,
    state: &mut ListState,
    is_active: bool,
) {
    let asset_layout = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Spread info (最上面，作為參考)
            Constraint::Length(10), // Asks (貼近底部)
            Constraint::Length(10), // Bids (最下面，緊鄰底部)
            Constraint::Length(15), // Recent trades (Activity 在最下面)
        ])
        .split(area);

    // Spread info (最上面) - 價差信息作為參考點
    if let (Some(bid), Some(ask)) = (data.best_bid, data.best_ask) {
        let spread = ask - bid;
        let spread_pct = (spread / bid) * Decimal::from(100);
        let spread_text = format!("Bid: ${:.4} | Ask: ${:.4} | Spread: ${:.4} ({:.2}%)", 
            bid, ask, spread, spread_pct);
        let spread_para = Paragraph::new(spread_text)
            .style(Style::default().fg(Color::Cyan))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(format!("{} Spread", label)));
        f.render_widget(spread_para, asset_layout[0]);
    } else {
        let spread_para = Paragraph::new("Waiting for data...")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(format!("{} Spread", label)));
        f.render_widget(spread_para, asset_layout[0]);
    }

    // Asks (貼近底部) - 賣單，將最接近 Bids 最大值的記錄放在底部，然後往上展示
    let mut asks_sorted: Vec<_> = data.asks.iter().collect();
    
    // 如果有 best_bid，找到最接近的 ask 價格，從那裡開始往上展示
    if let Some(best_bid) = data.best_bid {
        // 找到最接近 best_bid 的 ask 價格（應該 >= best_bid）
        let asks_above_bid: Vec<_> = asks_sorted.iter()
            .filter(|ask| ask.price >= best_bid)
            .collect();
        
        if !asks_above_bid.is_empty() {
            // 找到最接近 best_bid 的 ask 價格
            let closest_price = asks_above_bid.iter()
                .min_by(|a, b| {
                    let diff_a = (a.price - best_bid).abs();
                    let diff_b = (b.price - best_bid).abs();
                    diff_a.cmp(&diff_b)
                })
                .map(|ask| ask.price);
            
            if let Some(closest) = closest_price {
                // 重新排序：
                // 1. 所有 >= closest 的 asks 按價格從低到高排序（最接近的在底部，更高的往上）
                // 2. 所有 < closest 的 asks 按價格從高到低排序，放在最前面
                asks_sorted.sort_by(|a, b| {
                    let a_above_closest = a.price >= closest;
                    let b_above_closest = b.price >= closest;
                    
                    if a_above_closest && b_above_closest {
                        // 都在 closest 之上，按價格從低到高排序（最接近的在底部）
                        a.price.cmp(&b.price)
                    } else if a_above_closest {
                        std::cmp::Ordering::Greater // a 在後面（底部）
                    } else if b_above_closest {
                        std::cmp::Ordering::Less // b 在後面（底部）
                    } else {
                        // 都不在 closest 之上，按價格從高到低排序（放在前面）
                        b.price.cmp(&a.price)
                    }
                });
            } else {
                // 沒有找到最接近的，按價格從高到低排序
                asks_sorted.sort_by(|a, b| b.price.cmp(&a.price));
            }
        } else {
            // 沒有 >= best_bid 的 ask，按價格從高到低排序
            asks_sorted.sort_by(|a, b| b.price.cmp(&a.price));
        }
    } else {
        // 沒有 best_bid，按價格從高到低排序
        asks_sorted.sort_by(|a, b| b.price.cmp(&a.price));
    }
    
    // 反轉 Asks 順序（top down 反轉）
    asks_sorted.reverse();
    
    // Calculate max sizes for alignment
    let max_size_width = asks_sorted.iter()
        .take(10)
        .map(|ask| format_size_with_commas(ask.size).len())
        .max()
        .unwrap_or(15);
    
    let max_total_width = asks_sorted.iter()
        .take(10)
        .map(|ask| {
            let total = ask.price * ask.size;
            format_dollar_amount(total).len()
        })
        .max()
        .unwrap_or(12);
    
    let asks: Vec<ListItem> = asks_sorted.iter().take(10).map(|ask| {
        let price = format_price_as_cents(ask.price);
        let size = format_size_with_commas(ask.size);
        let size_aligned = format!("{:>width$}", size, width = max_size_width);
        let total = ask.price * ask.size;
        let total_str = format_dollar_amount(total);
        let total_aligned = format!("{:>width$}", total_str, width = max_total_width);
        
        let line = Line::from(vec![
            Span::styled(price, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::raw(size_aligned),
            Span::raw("  "),
            Span::raw(total_aligned),
        ]);
        ListItem::new(line)
    }).collect();

    let asks_list = List::new(asks)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("📉 {} Asks ({})", label, data.asks.len())),
        );
    f.render_widget(asks_list, asset_layout[1]);

    // Bids (最下面，緊鄰底部) - 買單，價格從高到低排序（從上到下）
    let mut bids_sorted: Vec<_> = data.bids.iter().collect();
    bids_sorted.sort_by(|a, b| b.price.cmp(&a.price)); // 降序：高價在上
    
    // Calculate max sizes for alignment
    let max_size_width = bids_sorted.iter()
        .take(10)
        .map(|bid| format_size_with_commas(bid.size).len())
        .max()
        .unwrap_or(15);
    
    let max_total_width = bids_sorted.iter()
        .take(10)
        .map(|bid| {
            let total = bid.price * bid.size;
            format_dollar_amount(total).len()
        })
        .max()
        .unwrap_or(12);
    
    let bids: Vec<ListItem> = bids_sorted.iter().take(10).map(|bid| {
        let price = format_price_as_cents(bid.price);
        let size = format_size_with_commas(bid.size);
        let size_aligned = format!("{:>width$}", size, width = max_size_width);
        let total = bid.price * bid.size;
        let total_str = format_dollar_amount(total);
        let total_aligned = format!("{:>width$}", total_str, width = max_total_width);
        
        let line = Line::from(vec![
            Span::styled(price, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::raw(size_aligned),
            Span::raw("  "),
            Span::raw(total_aligned),
        ]);
        ListItem::new(line)
    }).collect();

    let bids_list = List::new(bids)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("📈 {} Bids ({})", label, data.bids.len())),
        );
    f.render_widget(bids_list, asset_layout[2]);

    // Recent trades - 显示更多交易（从 5 增加到 12），包含可点击的 hash
    let trades: Vec<ListItem> = data.recent_trades.iter().take(12).map(|(time, trade, hash)| {
        let side_str = match trade.side {
            polysqueeze::types::Side::BUY => "B",
            polysqueeze::types::Side::SELL => "S",
        };
        let side_color = match trade.side {
            polysqueeze::types::Side::BUY => Color::Green,
            polysqueeze::types::Side::SELL => Color::Red,
        };
        
        let time_str = time.format("%H:%M:%S").to_string();
        let price_str = format!("${:.4}", trade.price);
        let size_str = format_size_with_commas(trade.size);
        
        let mut spans = vec![
            Span::styled(time_str, Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
            Span::styled(side_str, Style::default().fg(side_color).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(price_str, Style::default().fg(Color::Yellow)),
            Span::raw(" "),
            Span::raw(size_str),
        ];

        // Add hash link if available
        if let Some(h) = hash {
            let short_hash = if h.len() > 8 {
                format!("{}..{}", &h[..4], &h[h.len()-4..])
            } else {
                h.clone()
            };
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("🔗{}", short_hash),
                Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED),
            ));
        }
        
        ListItem::new(Line::from(spans))
    }).collect();

    let trades_list = List::new(trades)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("💰 {} Activity ({})", label, data.recent_trades.len())),
        )
        .highlight_style(
            Style::default()
                .bg(if is_active { Color::Blue } else { Color::DarkGray })
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(if is_active { ">> " } else { "   " });
    
    f.render_stateful_widget(trades_list, asset_layout[3], state);
}

/// 处理 WebSocket 事件 (保留用于向后兼容)
async fn handle_events(
    client: &mut WssMarketClient,
    event_limit: Option<usize>,
) -> Result<()> {
    if let Some(limit) = event_limit {
        // 限制事件数量
        for _ in 0..limit {
            match client.next_event().await {
                Ok(WssMarketEvent::PriceChange(change)) => {
                    println!(
                        "💹 price_change for {}: {:?}",
                        change.market, change.price_changes
                    );
                }
                Ok(WssMarketEvent::Book(book)) => {
                    println!(
                        "📖 book {} bids={} asks={}",
                        book.market,
                        book.bids.len(),
                        book.asks.len()
                    );
                }
                Ok(WssMarketEvent::TickSizeChange(change)) => {
                    println!(
                        "🔧 tick size change {} from {} to {}",
                        change.market, change.old_tick_size, change.new_tick_size
                    );
                }
                Ok(WssMarketEvent::LastTrade(trade)) => {
                    println!(
                        "💰 last_trade {} {:?}@{}",
                        trade.market, trade.side, trade.price
                    );
                }
                Err(err) => {
                    eprintln!("❌ stream error: {}", err);
                    break;
                }
            }
        }
    } else {
        // 持续接收事件（无限循环）
        loop {
            match client.next_event().await {
                Ok(WssMarketEvent::PriceChange(change)) => {
                    println!(
                        "💹 price_change for {}: {:?}",
                        change.market, change.price_changes
                    );
                }
                Ok(WssMarketEvent::Book(book)) => {
                    println!(
                        "📖 book {} bids={} asks={}",
                        book.market,
                        book.bids.len(),
                        book.asks.len()
                    );
                }
                Ok(WssMarketEvent::TickSizeChange(change)) => {
                    println!(
                        "🔧 tick size change {} from {} to {}",
                        change.market, change.old_tick_size, change.new_tick_size
                    );
                }
                Ok(WssMarketEvent::LastTrade(trade)) => {
                    println!(
                        "💰 last_trade {} {:?}@{}",
                        trade.market, trade.side, trade.price
                    );
                }
                Err(err) => {
                    eprintln!("❌ stream error: {}", err);
                    break;
                }
            }
        }
    }
    
    Ok(())
}


/// Open a URL in the default browser
fn open_url(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(url)
            .spawn()
            .map_err(|e| PolyError::internal(format!("Failed to open URL: {}", e), e))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| PolyError::internal(format!("Failed to open URL: {}", e), e))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| PolyError::internal(format!("Failed to open URL: {}", e), e))?;
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return Err(PolyError::validation("Unsupported platform for opening URLs"));
    }
    Ok(())
}

fn derive_asset_ids(market: &Market) -> Option<Vec<String>> {
    if !market.clob_token_ids.is_empty() {
        return Some(market.clob_token_ids.clone());
    }

    let ids = market
        .tokens
        .iter()
        .map(|token| token.token_id.clone())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();

    if ids.is_empty() { None } else { Some(ids) }
}

/// TUI for selecting a market from a list sorted by liquidity
async fn select_market_tui(markets: &[Market]) -> Result<Market> {
    // Sort markets by liquidity (highest first)
    let mut sorted_markets: Vec<&Market> = markets
        .iter()
        .filter(|m| m.active && !m.closed && m.liquidity_num.is_some())
        .collect();
    
    sorted_markets.sort_by(|a, b| {
        let liq_a = a.liquidity_num.unwrap_or_default();
        let liq_b = b.liquidity_num.unwrap_or_default();
        liq_b.cmp(&liq_a) // Descending order
    });

    if sorted_markets.is_empty() {
        return Err(PolyError::validation("No active markets with liquidity found"));
    }

    // Setup terminal
    enable_raw_mode().map_err(|e| PolyError::internal(format!("Failed to enable raw mode: {}", e), e))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| PolyError::internal(format!("Failed to setup terminal: {}", e), e))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend).map_err(|e| PolyError::internal(format!("Failed to create terminal: {}", e), e))?;

    let mut state = ListState::default();
    state.select(Some(0));

    let result = loop {
        terminal.draw(|f| ui_market_list(f, &sorted_markets, &mut state)).map_err(|e| PolyError::internal(format!("Failed to draw terminal: {}", e), e))?;

        if let Event::Key(key) = event::read().map_err(|e| PolyError::internal(format!("Terminal I/O error: {}", e), e))? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        break Err(PolyError::validation("User cancelled market selection"));
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let i = state.selected().unwrap_or(0);
                        if i < sorted_markets.len().saturating_sub(1) {
                            state.select(Some(i + 1));
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let i = state.selected().unwrap_or(0);
                        if i > 0 {
                            state.select(Some(i - 1));
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(selected) = state.selected() {
                            break Ok(sorted_markets[selected].clone());
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    // Restore terminal
    disable_raw_mode().map_err(|e| PolyError::internal(format!("Failed to disable raw mode: {}", e), e))?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    ).map_err(|e| PolyError::internal(format!("Failed to restore terminal: {}", e), e))?;
    terminal.show_cursor().map_err(|e| PolyError::internal(format!("Failed to show cursor: {}", e), e))?;

    result
}

/// TUI for selecting Yes or No asset from a market
async fn select_asset_tui(market: &Market) -> Result<String> {
    let asset_ids = derive_asset_ids(market);
    
    if asset_ids.is_none() || asset_ids.as_ref().unwrap().len() < 2 {
        return Err(PolyError::validation(
            "Market does not have Yes/No tokens available",
        ));
    }

    let yes_token = &market.tokens[0];
    let no_token = &market.tokens[1];
    let assets: Vec<(&str, &str, &str)> = vec![
        ("Yes", yes_token.token_id.as_str(), yes_token.outcome.as_str()),
        ("No", no_token.token_id.as_str(), no_token.outcome.as_str()),
    ];

    // Setup terminal
    enable_raw_mode().map_err(|e| PolyError::internal(format!("Failed to enable raw mode: {}", e), e))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| PolyError::internal(format!("Failed to setup terminal: {}", e), e))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend).map_err(|e| PolyError::internal(format!("Failed to create terminal: {}", e), e))?;

    let mut state = ListState::default();
    state.select(Some(0));

    let result = loop {
        terminal.draw(|f| ui_asset_selection(f, market, &assets, &mut state)).map_err(|e| PolyError::internal(format!("Failed to draw terminal: {}", e), e))?;

        if let Event::Key(key) = event::read().map_err(|e| PolyError::internal(format!("Terminal I/O error: {}", e), e))? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        break Err(PolyError::validation("User cancelled asset selection"));
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let i = state.selected().unwrap_or(0);
                        if i < assets.len().saturating_sub(1) {
                            state.select(Some(i + 1));
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let i = state.selected().unwrap_or(0);
                        if i > 0 {
                            state.select(Some(i - 1));
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(selected) = state.selected() {
                            break Ok(assets[selected].1.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    };

    // Restore terminal
    disable_raw_mode().map_err(|e| PolyError::internal(format!("Failed to disable raw mode: {}", e), e))?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    ).map_err(|e| PolyError::internal(format!("Failed to restore terminal: {}", e), e))?;
    terminal.show_cursor().map_err(|e| PolyError::internal(format!("Failed to show cursor: {}", e), e))?;

    result
}

/// Render the market list UI
fn ui_market_list(f: &mut Frame, markets: &[&Market], state: &mut ListState) {
    let size = f.area();
    
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(0),    // List
            Constraint::Length(3), // Footer
        ])
        .split(size);

    // Header
    let header = Paragraph::new("📊 選擇市場 (Select Market)")
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    // Find maximum liquidity value to determine right-alignment width
    let max_liquidity = markets
        .iter()
        .filter_map(|m| m.liquidity_num)
        .max()
        .unwrap_or_default();
    
    let max_liquidity_str = format!("${}", format_with_commas(max_liquidity));
    let liquidity_width = max_liquidity_str.len();

    // Market list
    let items: Vec<ListItem> = markets
        .iter()
        .enumerate()
        .map(|(idx, market)| {
            let liquidity_str = market
                .liquidity_num
                .map(|l| format!("${}", format_with_commas(l)))
                .unwrap_or_else(|| "N/A".to_string());
            
            // Right-align liquidity to match the maximum width
            let liquidity = format!("{:>width$}", liquidity_str, width = liquidity_width);
            
            let category = market
                .category
                .as_ref()
                .map(|c| format!("[{}] ", c))
                .unwrap_or_default();

            let question = if market.question.len() > 60 {
                format!("{}...", &market.question[..57])
            } else {
                market.question.clone()
            };

            let line = Line::from(vec![
                Span::styled(
                    format!("{:3}. ", idx + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("💧 {} │ ", liquidity),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(category, Style::default().fg(Color::Magenta)),
                Span::raw(question),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Active Markets ({} total) - Sorted by Liquidity", markets.len())),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, chunks[1], state);

    // Footer
    let footer = Paragraph::new("↑/↓: Navigate | Enter: Select | Q/ESC: Cancel")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}

/// Render the asset selection UI
fn ui_asset_selection(
    f: &mut Frame,
    market: &Market,
    assets: &[(&str, &str, &str)],
    state: &mut ListState,
) {
    let size = f.area();

    let chunks = Layout::default()
        .constraints([
            Constraint::Length(5), // Market info
            Constraint::Min(0),    // Asset list
            Constraint::Length(3), // Footer
        ])
        .split(size);

    // Market info header
    let market_info = vec![
        Line::from(vec![
            Span::styled("Question: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(&market.question),
        ]),
        Line::from(vec![
            Span::styled("Condition ID: ", Style::default().fg(Color::Cyan)),
            Span::raw(&market.condition_id),
        ]),
        Line::from(vec![
            Span::styled("Liquidity: ", Style::default().fg(Color::Green)),
            Span::raw(
                market
                    .liquidity_num
                    .map(|l| format!("${}", format_with_commas(l)))
                    .unwrap_or_else(|| "N/A".to_string()),
            ),
        ]),
    ];

    let info_block = Paragraph::new(market_info)
        .block(Block::default().borders(Borders::ALL).title("Market Information"))
        .wrap(Wrap { trim: true });
    f.render_widget(info_block, chunks[0]);

    // Asset list
    let items: Vec<ListItem> = assets
        .iter()
        .enumerate()
        .map(|(idx, (label, token_id, outcome))| {
            let line = Line::from(vec![
                Span::styled(
                    format!("{:2}. ", idx + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{} ", label),
                    Style::default().fg(if *label == "Yes" { Color::Green } else { Color::Red }).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("({}) ", outcome)),
                Span::styled(
                    format!("Token: {}", &token_id[..20]),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("選擇資產 (Select Asset)"),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, chunks[1], state);

    // Footer
    let footer = Paragraph::new("↑/↓: Navigate | Enter: Select | Q/ESC: Cancel")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
