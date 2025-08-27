// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// -------- Imports --------
use std::{error::Error as STDError, ffi::OsString, fs::{self, rename}, sync::{Arc, Mutex, RwLock}, thread::{self, Thread}};
use savefile::{load_file, save_file};
use savefile_derive::Savefile;
use slint::{Model, ModelRc, SharedString, ToSharedString, VecModel};
use qruhear::{RUHear, RUBuffers, rucallback};
use hound::{WavWriter, SampleFormat, WavSpec};

slint::include_modules!();

// -------- Enums --------
// Errors
#[derive(Clone, Copy, PartialEq)]
enum Error {
    SaveError,
    LoadError,
    RecordError,
    WriteError,
    ReadError,
    RenameError,
    DeleteError,
}

impl Error {
    fn get_text(kind: Error) -> SharedString {
        match kind {
            Error::SaveError => SharedString::from("Failed to save data ... Reverting to previous save"),
            Error::LoadError => SharedString::from("Data doesn't exist ... Creating new file"),
            Error::RecordError => SharedString::from("Recording failed ... Please try again"),
            Error::WriteError => SharedString::from("Failed to write audio"),
            Error::ReadError => SharedString::from("Failed to read files"),
            Error::RenameError => SharedString::from("Failed to rename file"),
            Error::DeleteError => SharedString::from("Failed to delete file"),
        }
    }
}

// Successes
enum Success {
    SaveSuccess,
    RecordSuccess,
    RenameSuccess,
    DeleteSuccess,
}

// File stuff
#[derive(PartialEq)]
enum File {
    Names(Vec<String>),
}

impl File {
    fn search(path: &str, extension: &str) -> Result<File, Error> {
        let mut names = vec![];
        match fs::read_dir(path) {
            Ok(directories) => {
                for entry in directories {
                    match entry {
                        Ok(directory) => {
                            let path = directory.path();

                            if path.is_file() {
                                if let Some(file_type) = path.extension() {
                                    if file_type == extension {
                                        let file_name = match path.file_name() {
                                            Some(value) => value.to_owned(),
                                            None => OsString::from("Couldn't read name"),
                                        };
                                        names.push(match file_name.into_string() {
                                            Ok(value) => {
                                                File::truncate(value)
                                            },
                                            Err(_) => String::from("Couldn't read name"),
                                        });
                                    }
                                }
                            }
                        },
                        Err(_) => {
                            return Err(Error::ReadError);
                        },
                    }
                }
                names.sort();
                Ok(File::Names(names))
            },
            Err(_) => Err(Error::ReadError),
        }
    }

    fn truncate(mut name: String) -> String {
        let mut length = name.len() - 1;
        loop {
            if name.ends_with(".") {
                name.remove(length);
                break;
            } else {
                if length == 1 {
                    name = String::from("Invalid file extension");
                }
                name.remove(length);
                length -= 1;
            }
        }

        String::from(name)
    }

    fn rename(names: Vec<String>) -> Result<Success, Error> {
        let old = match File::search("./", "wav") {
            Ok(File::Names(value)) => value,
            Err(_) => vec![String::from("Couldn't read names")],
        };

        for recording in 0..names.len() {
            match rename(format!("./{}.wav", old[recording]), format!("{}.wav", names[recording])) {
                Ok(_) => {
                },
                Err(error) => {
                    println!("{}", error);
                    return Err(Error::RenameError);
                },
            };
        }

        Ok(Success::RenameSuccess)
    }

    fn delete(names: Vec<String>) {

    }
}

// -------- Structs --------
// Index data for Settings struct
struct IndexData {
    preset_length: usize,
    recording_length: usize,
}

// Preset data
#[derive(Savefile)]
struct Preset {
    name: String,
    bass: i32,
    vocals: i32,
    treble: i32,
    gain: i32,
    reverb: i32,
    crush: i32,
}

impl Preset {
    fn from(values: [i32; 6]) -> Preset {
        Preset {
            name: String::from("New Preset"),
            bass: values[0],
            vocals: values[1],
            treble: values[2],
            gain: values[3],
            reverb: values[4],
            crush: values[5],
        }
    }

    fn get_names(list: &Vec<Preset>, length: &usize) -> ModelRc<SharedString> {
        let mut preset_names: Vec<SharedString> = vec![];
        for preset in 0..*length {
            preset_names.push(list[preset].name.to_shared_string());
        }
        ModelRc::new(VecModel::from(preset_names))
    }

    fn get_values(list: &Vec<Preset>, length: &usize) -> ModelRc<ModelRc<i32>> {
        let mut all_preset_values: Vec<ModelRc<i32>> = vec![];
        for values in 0..*length {
            let mut preset_values: Vec<i32> = vec![];
            
            preset_values.push(list[values].bass);
            preset_values.push(list[values].vocals);
            preset_values.push(list[values].treble);
            preset_values.push(list[values].gain);
            preset_values.push(list[values].reverb);
            preset_values.push(list[values].crush);

            all_preset_values.push(ModelRc::new(VecModel::from(preset_values)));
        }
        ModelRc::new(VecModel::from(all_preset_values))
    }
}

// Recording data
#[derive(Savefile)]
struct Recording {
    bass: i32,
    vocals: i32,
    treble: i32,
    gain: i32,
    reverb: i32,
    crush: i32,
}

impl Recording {
    fn new() -> Recording {
        Recording {
            bass: 0,
            vocals: 0,
            treble: 0,
            gain: 0,
            reverb: 0,
            crush: 0,
        }
    }

    fn from(values: [i32; 6]) -> Recording {
        Recording {
            bass: values[0],
            vocals: values[1],
            treble: values[2],
            gain: values[3],
            reverb: values[4],
            crush: values[5],
        }
    }

    fn parse(recording: &Recording) -> [i32; 6] {
        let mut list = [0, 0, 0, 0, 0, 0];

        list[0] = recording.bass;
        list[1] = recording.vocals;
        list[2] = recording.treble;
        list[3] = recording.gain;
        list[4] = recording.reverb;
        list[5] = recording.crush;

        list
    }

    fn get_names(list: &Vec<String>) -> ModelRc<SharedString> {
        let mut new_list = vec![];

        for recording in 0..list.len() {
            new_list.push(list[recording].to_shared_string());
        }

        ModelRc::new(VecModel::from(new_list))
    }

    fn get_values(list: &Vec<Recording>, length: &usize) -> ModelRc<ModelRc<i32>> {
        let mut all_recording_values: Vec<ModelRc<i32>> = vec![];
        for values in 0..*length {
            let mut recording_values: Vec<i32> = vec![];
            
            recording_values.push(list[values].bass);
            recording_values.push(list[values].vocals);
            recording_values.push(list[values].treble);
            recording_values.push(list[values].gain);
            recording_values.push(list[values].reverb);
            recording_values.push(list[values].crush);

            all_recording_values.push(ModelRc::new(VecModel::from(recording_values)));
        }
        ModelRc::new(VecModel::from(all_recording_values))
    }

    fn update_values(saved_list: &Vec<Recording>, names_list: &Vec<String>, length: &usize) -> Vec<Recording> {
        let mut recording_values= vec![];
        for values in 0..names_list.len() {
            if values < *length {
                recording_values.push(Recording::from(Recording::parse(&saved_list[values])));
            } else {
                recording_values.push(Recording::new());
            }
        }

        recording_values
    }

    fn edited(record: &Recording, list: [i32; 6]) -> bool {
        if record.bass == list[0]
        && record.vocals == list[1]
        && record.treble == list[2]
        && record.gain == list[3]
        && record.reverb == list[4]
        && record.crush == list[5] {
            false
        } else {
            true
        }
    }

    fn get_renamed_names(list: ModelRc<SharedString>, length: &usize) -> Vec<String> {
        let mut new = vec![];
        for name in 0..*length {
            new.push(String::from(list.row_data(name).unwrap()));
        }

        new
    }
}

// All settings data
#[derive(Savefile)]
struct Settings {
    presets: Vec<Preset>,
    recordings: Vec<Recording>,
}

impl Settings {
    fn new() -> Settings {
        Settings {
            presets: vec![],
            recordings: vec![],
        }
    }

    fn get_index_data(&self) -> IndexData {
        IndexData {
            preset_length: self.presets.len(),
            recording_length: self.recordings.len(),
        }
    }

    fn sync(&mut self, new: &AppWindow) {
        let mut index_data = self.get_index_data();

        let mut dials = [0, 0, 0, 0, 0, 0];
        for index in 0..6 {
            dials[index] = new.get_dial_values().row_data(index).unwrap();
        }

        // Check for new preset creation
        if new.get_new_preset() {
            self.presets.push(Preset::from(dials));
        }

        // Check for preset deletion
        if new.get_delete_preset() {
            self.presets.remove(new.get_deleted_preset() as usize);
        }

        // Check for preset rename
        if new.get_rename_preset() {
            for preset in 0..index_data.preset_length {
                self.presets[preset].name = String::from(match new.get_preset_names().row_data(preset) {
                    Some(name) => name,
                    None => SharedString::from("New Preset"),
                });
            }
        }

        // Check for recording names
        if new.get_new_recording() {
            let recording_names = match File::search("./", "wav") {
                Ok(File::Names(value)) => value,
                Err(_) => vec![String::from("Couldn't read files")],
            };
    
            self.recordings = Recording::update_values(&self.recordings, &recording_names, &index_data.recording_length);
    
            // Sends a list of recording names to the UI to be displayed
            new.set_recording_names(Recording::get_names(&recording_names));

            new.set_new_recording(false);
            index_data = self.get_index_data();
        }

        // Check for recording edits
        if index_data.recording_length > 0 {
            if Recording::edited(&self.recordings[new.get_current_recording() as usize], dials) {
                self.recordings[new.get_current_recording() as usize] = Recording::from(dials);
            }
        }

        // Check for recording deletion
        if new.get_delete_recording() {
            self.recordings.remove(new.get_deleted_recording() as usize);
        }
    }
}

// Keeps track of the settings and the recording thread
struct Tracker {
    settings: Arc<RwLock<Settings>>,
    recorder: Arc<Mutex<Option<Thread>>>,
}

impl Tracker {
    fn new(settings: Settings) -> Tracker {
        Tracker {
            settings: Arc::new(RwLock::new(settings)),
            recorder: Arc::new(Mutex::new(None)),
        }
    }

    fn record(self: &Arc<Self>) -> Result<Success, Error> {
        let current_thread = Arc::clone(self);

        let state = match thread::Builder::new().name(String::from("Recorder")).spawn(move || {
            *current_thread.recorder.lock().unwrap() = Some(thread::current());

            let audio_spec = WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: SampleFormat::Float,
            };

            let recording_amount = match File::search("./", "wav") {
                Ok(File::Names(value)) => value.len(),
                Err(_) => 0,
            };

            let mut writer = match WavWriter::create(format!("Recording {}.wav", recording_amount + 1), audio_spec) {
                Ok(value) => value,
                Err(_) => {
                    return Err(Error::WriteError);
                }
            };
            
            let record_edit = move |data: RUBuffers| {
                // println!("{:?}", data);
                for sample in &data[1] {
                    writer.write_sample(*sample).unwrap();
                }
            };
            
            let callback = rucallback!(record_edit);
            
            let mut recorder = RUHear::new(callback);

            match recorder.start() {
                Ok(_) => Success::RecordSuccess,
                Err(_) => {
                    return Err(Error::RecordError);
                },
            };

            thread::park();

            match recorder.stop() {
                Ok(_) => Success::RecordSuccess,
                Err(_) => {
                    return Err(Error::RecordError);
                },
            };

            return Ok(Success::RecordSuccess);
        }) {
            Ok(_) => Ok(Success::RecordSuccess),
            Err(_) => Err(Error::RecordError),
        };

        state
    }

    fn stop(self: &Arc<Self>) {
        let current_thread = Arc::clone(self);

        if let Some(recorder) = self.recorder.lock().unwrap().as_ref() {
            recorder.unpark();
        }

        *current_thread.recorder.lock().unwrap() = None;
    }
}

// -------- Functions --------
fn save(data: &Settings) -> Result<Success, Error> {
    match save_file("settings.bin", 0, data) {
        Ok(_) => Ok(Success::SaveSuccess),
        Err(_) => Err(Error::SaveError),
    }
}

fn load() -> Result<Settings, Error> {
    match load_file("settings.bin", 0) {
        Ok(value) => Ok(value),
        Err(_) => Err(Error::LoadError),
    }
}

fn main() -> Result<(), Box<dyn STDError>> {
    let ui = AppWindow::new()?;

    let mut load_error = None;

    // Creates a variable that can be used across threads and move blocks and can be read from without locking
    let tracker = Arc::new(Tracker::new(match load() {
        Ok(value) => value,
        Err(error) => {
            let _ = save(&Settings::new());
            load_error = Some(error);
            Settings::new()
        }
    }));

    ui.on_startup({
        let ui_handle = ui.as_weak();

        let startup_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            match load_error {
                Some(value) => {
                    ui.set_error_notification(Error::get_text(value));
                    ui.set_error_recieved(true);
                    load_error = None;
                },
                None => {

                }
            }

            // Acquires write access to the loaded data
            let mut settings = startup_ref_count.write().unwrap();

            let mut index_data = settings.get_index_data();

            // Sends a list of preset names to the UI to be displayed
            ui.set_preset_names(Preset::get_names(&settings.presets, &index_data.preset_length));

            // Sends a nested list of preset values to the UI to be displayed
            ui.set_preset_values(Preset::get_values(&settings.presets, &index_data.preset_length));

            if ui.get_started() {
                settings.sync(&ui);

                // Check for recording names
                let recording_names = match File::search("./", "wav") {
                    Ok(File::Names(value)) => value,
                    Err(_) => vec![String::from("Couldn't read files")],
                };

                // Sends a list of recording names to the UI to be displayed
                ui.set_recording_names(Recording::get_names(&recording_names));

                ui.set_started(false);
                index_data = settings.get_index_data();
            }

            // Sends a nested list of recording edits to the UI to be displayed
            if index_data.recording_length > 0 {
                ui.set_recording_values(Recording::get_values(&settings.recordings, &index_data.recording_length));
            }
        }
    });

    ui.on_update_and_save({
        let ui_handle = ui.as_weak();

        let update_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            // This block is used to drop the write lock on the stored data as soon as the last write is completed
            // This frees it to be used elsewhere slightly quicker
            {
                // Acquires write access to the loaded data
                let mut settings = update_ref_count.write().unwrap();
                settings.sync(&ui);
            }

            ui.invoke_startup();

            // Aquires read access to the loaded data
            let settings = update_ref_count.read().unwrap();
            let _ = save(&settings);
        }
    });

    ui.on_record({
        let ui_handle = ui.as_weak();

        let tracker_ref_count = Arc::clone(&tracker);
        let poison_count = tracker.clone();

        move || {
            let ui = ui_handle.unwrap();

            if ui.get_recording() {
                match tracker_ref_count.record() {
                    Ok(_) => (),
                    Err(error) => {
                        ui.set_error_notification(Error::get_text(error));
                        ui.set_error_recieved(true);
                        poison_count.recorder.clear_poison();
                        ui.set_recording(false);
                        tracker_ref_count.stop();
                    }
                }
            } else {
                tracker_ref_count.stop();
                ui.invoke_update_and_save();
            }
        }
    });

    ui.on_rename_recording({
        let ui_handle = ui.as_weak();

        let rename_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = rename_ref_count.read().unwrap();

            let index_data = settings.get_index_data();

            let names = Recording::get_renamed_names(ui.get_recording_names(), &index_data.recording_length);

            match File::rename(names) {
                Ok(_) => {
                },
                Err(error) => {
                    ui.set_error_notification(Error::get_text(error));
                    ui.set_error_recieved(true);
                }
            };
        }
    });

    // ui.on_name({
    //     let ui_handle = ui.as_weak();
    //     move || {
    //         let ui = ui_handle.unwrap();
    //     }
    // });

    ui.run()?;

    Ok(())
}
