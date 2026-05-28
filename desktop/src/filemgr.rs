use std::fs;
use std::path::{Path, PathBuf};

pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

pub struct FileManager {
    cwd: PathBuf,
    entries: Vec<FileEntry>,
    selected: usize,
    scroll_offset: usize,
}

impl FileManager {
    pub fn new(start_path: &str) -> Self {
        let cwd = PathBuf::from(start_path);
        let mut fm = FileManager {
            cwd,
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
        };
        fm.refresh();
        fm
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn refresh(&mut self) {
        self.entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;

        // Add parent directory entry (..)
        if self.cwd.parent().is_some() {
            self.entries.push(FileEntry {
                name: String::from(".."),
                is_dir: true,
                size: 0,
            });
        }

        // List directory contents
        if let Ok(entries) = fs::read_dir(&self.cwd) {
            let mut files: Vec<FileEntry> = entries
                .filter_map(|entry| {
                    entry.ok().and_then(|e| {
                        let name = e.file_name().into_string().ok()?;
                        let metadata = e.metadata().ok()?;
                        Some(FileEntry {
                            name,
                            is_dir: metadata.is_dir(),
                            size: metadata.len(),
                        })
                    })
                })
                .collect();

            // Sort: directories first, then alphabetically
            files.sort_by(|a, b| {
                if a.is_dir != b.is_dir {
                    b.is_dir.cmp(&a.is_dir)
                } else {
                    a.name.cmp(&b.name)
                }
            });

            self.entries.extend(files);
        }
    }

    pub fn navigate(&mut self, name: &str) {
        if name == ".." {
            if let Some(parent) = self.cwd.parent() {
                self.cwd = parent.to_path_buf();
                self.refresh();
            }
        } else {
            let new_path = self.cwd.join(name);
            if new_path.is_dir() {
                self.cwd = new_path;
                self.refresh();
            }
        }
    }

    pub fn select_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    pub fn select_down(&mut self) {
        if self.selected < self.entries.len().saturating_sub(1) {
            self.selected += 1;
            let visible_rows = 20;
            if self.selected >= self.scroll_offset + visible_rows {
                self.scroll_offset = self.selected.saturating_sub(visible_rows - 1);
            }
        }
    }

    pub fn open_selected(&mut self) {
        if self.selected < self.entries.len() {
            let name = self.entries[self.selected].name.clone();
            self.navigate(&name);
        }
    }

    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn cwd_display(&self) -> String {
        self.cwd
            .to_string_lossy()
            .to_string()
    }

    fn format_size(size: u64) -> String {
        if size < 1024 {
            format!("{}B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1}K", size as f64 / 1024.0)
        } else if size < 1024 * 1024 * 1024 {
            format!("{:.1}M", size as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1}G", size as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    pub fn render_to_string(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("Location: {}\n", self.cwd_display()));
        output.push_str("────────────────────────────────────────────\n");

        let visible_rows = 20;
        let end = (self.scroll_offset + visible_rows).min(self.entries.len());

        for (i, entry) in self.entries[self.scroll_offset..end].iter().enumerate() {
            let actual_idx = self.scroll_offset + i;
            let marker = if actual_idx == self.selected { ">" } else { " " };
            let type_marker = if entry.is_dir { "/" } else { " " };
            let size_str = if entry.is_dir {
                String::new()
            } else {
                Self::format_size(entry.size)
            };

            output.push_str(&format!(
                "{} {:<40} {:>8}\n",
                marker,
                format!("{}{}", entry.name, type_marker),
                size_str
            ));
        }

        output.push_str("────────────────────────────────────────────\n");
        if self.entries.len() > visible_rows {
            output.push_str(&format!(
                "({} of {} items)\n",
                end,
                self.entries.len()
            ));
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_manager_new() {
        let fm = FileManager::new("/tmp");
        assert_eq!(fm.cwd(), Path::new("/tmp"));
    }

    #[test]
    fn test_navigation() {
        let mut fm = FileManager::new("/");
        let initial = fm.cwd().to_path_buf();
        if let Some(first_dir) = fm
            .entries()
            .iter()
            .find(|e| e.is_dir && e.name != "..")
            .map(|e| e.name.clone())
        {
            fm.open_selected();
            // After opening a directory, cwd should change
            // (unless the entry doesn't exist or is not accessible)
        }
    }
}
