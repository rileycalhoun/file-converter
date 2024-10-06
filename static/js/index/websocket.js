
window.addEventListener('DOMContentLoaded', () => {
	let head = document.getElementsByTagName("head")[0];
	let session_id = head.getAttribute("session");
	let website_url = head.getAttribute("website_url");
	
	if (session_id == null) {
		console.log("session_id not found");
		return;
	}

	let socket = new WebSocket(`${website_url}/ws`);
	socket.addEventListener('open', () => {
		socket.send(`session-id;${session_id}`);
	});

	socket.addEventListener('message', (msg) => {
		if (msg.type == "message") {
			let data = msg.data;
			if (typeof data == 'string' || data instanceof String) {
				console.log(data);

				if (data.startsWith('job-completed;')) {
					let id = data.split(';')[1];
					window.location.href = `files/${id}`;
				} else if (data.startsWith('job_failed;')) {
					console.log('Something went wrong!');
				}
			}

		}
	});
});
