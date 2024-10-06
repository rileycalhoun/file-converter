
window.onload = async () => {
	let form = document.getElementById('convert');

	let status = document.getElementById('status');
	status.style.visibility = "hidden";

	let status_message = document.getElementById('status-message');
	
	form.addEventListener('submit', async function(event) {
		event.preventDefault();
		let response = await fetch(event.target.action, {
			method: 'POST',
			body: new FormData(form)
		});	

		let message = await response.text();
		status_message.textContent = message;

		let background_color, border_color;
		if (response.status == 200) {
			background_color = "var(--success-color";
			border_color = "var(--border-color)";
		} else {
			background_color = "var(--error-color)";
			border_color = "var(--error-border-color)";
		}

		status.style.backgroundColor = background_color;
		status.style.border = `5px, ${border_color}`;
		status.style.display = "block";
		status.style.visibility = "visible";
	});
}
