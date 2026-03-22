use {
    crossterm::event::{self, Event as CrosstermEvent, KeyEvent},
    std::time::Duration,
};

pub enum Event {
    Key(KeyEvent),
    Tick,
}

pub struct EventHandler {
    rx: std::sync::mpsc::Receiver<Event>,
}

impl EventHandler {
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let tick_tx = tx.clone();

        std::thread::spawn(move || {
            loop {
                if event::poll(tick_rate).unwrap_or(false)
                    && let Ok(CrosstermEvent::Key(key)) = event::read()
                    && tx.send(Event::Key(key)).is_err()
                {
                    break;
                }
            }
        });

        std::thread::spawn(move || {
            loop {
                std::thread::sleep(tick_rate);
                if tick_tx.send(Event::Tick).is_err() {
                    break;
                }
            }
        });

        Self { rx }
    }

    pub fn next(&self) -> Result<Event, std::sync::mpsc::RecvError> {
        self.rx.recv()
    }
}
